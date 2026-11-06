// Conservative CI routing. Unrecognized paths, events or checkout relationships run every check.
import { execFileSync } from 'node:child_process'
import { appendFileSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const shaPattern = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i
const ruleName =
  /(?:^|[._-])(?:agents?|claude|skills?|gemini|copilot|instructions?|prompts?|rules?|cursorrules|windsurfrules)(?:[._-]|$)/i
const semver = String.raw`\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?`
const versionField = new RegExp(String.raw`^  "version": "${semver}",?$`)
const internalRange = new RegExp(String.raw`^    "@ouraihub/aether-[a-z-]+": "\^?${semver}",?$`)
const cargoVersion = new RegExp(`^version = "(${semver})"$`)

// Release bumps only rewrite these; the publish workflow still runs every platform on the result.
function versionManifest(path) {
  if (path.split('/').some((part) => ruleName.test(part))) return false
  return (
    path === 'Cargo.toml' ||
    path === 'Cargo.lock' ||
    /^(?:packages|examples)\/[a-z0-9-]+\/package\.json$/.test(path) ||
    /^examples\/[a-z0-9-]+\/src-tauri\/tauri\.conf\.json$/.test(path)
  )
}

// In-place line replacements as [head line, old, new]; null when the patch does anything else.
export function replacedLines(patch) {
  const lines = patch.split('\n')
  let index = 0
  for (; index < lines.length && !lines[index].startsWith('@@'); index++)
    if (/^(?:(?:new|deleted) file|old|new|rename|copy|similarity|Binary)/.test(lines[index]))
      return null
  const changes = []
  while (index < lines.length) {
    const hunk = /^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(lines[index++])
    if (!hunk) return index === lines.length && lines[index - 1] === '' ? changes : null
    const count = Number(hunk[1] ?? 1)
    if (count === 0 || count !== Number(hunk[3] ?? 1)) return null
    const removed = lines.slice(index, index + count)
    const added = lines.slice(index + count, index + 2 * count)
    if (
      !removed.every((line) => line.startsWith('-')) ||
      !added.every((line) => line.startsWith('+'))
    )
      return null
    removed.forEach((line, offset) =>
      changes.push([Number(hunk[2]) + offset, line.slice(1), added[offset].slice(1)]),
    )
    index += 2 * count
  }
  return changes
}

function lockPackages(text) {
  return text
    .split('\n[[package]]\n')
    .slice(1)
    .map((block) => ({
      name: /^name = "([^"]+)"$/m.exec(block)?.[1],
      version: /^version = "([^"]+)"$/m.exec(block)?.[1],
      local: !/^source = /m.test(block),
    }))
}

// Only workspace members (no `source`) may change, and only their version lines.
export function lockBumpOnly(baseText, headText, changes) {
  if (!changes.every(([, before, after]) => cargoVersion.test(before) && cargoVersion.test(after)))
    return false
  const base = lockPackages(baseText)
  const head = lockPackages(headText)
  return (
    base.length > 0 &&
    base.length === head.length &&
    base.every(
      (item, index) =>
        item.name === head[index].name &&
        item.local === head[index].local &&
        (item.version === head[index].version || item.local),
    )
  )
}

// Version-only edits of package.json, tauri.conf.json and `[workspace.package]` in Cargo.toml.
export function manifestBumpOnly(path, headText, changes) {
  const head = headText.split('\n')
  return changes.every(([line, before, after]) => {
    if (path === 'Cargo.toml')
      return (
        cargoVersion.test(before) &&
        cargoVersion.test(after) &&
        head.slice(0, line - 1).findLast((text) => text.startsWith('[')) === '[workspace.package]'
      )
    const allowed = path.endsWith('tauri.conf.json')
      ? [versionField]
      : [versionField, internalRange]
    return allowed.some(
      (pattern) =>
        pattern.test(before) && pattern.test(after) && before.split(':')[0] === after.split(':')[0],
    )
  })
}

// Every member that inherited the old workspace version moves to the new one; nothing else moves.
function lockFollowsWorkspace(baseLock, headLock, from, to) {
  const head = lockPackages(headLock)
  return lockPackages(baseLock).every((item, index) => {
    const after = head[index]?.version
    return item.local && item.version === from ? after === to : after === item.version
  })
}

function pathScope(path) {
  if (typeof path !== 'string' || !path || path.includes('\\') || path.includes('\0')) return 'full'
  if (
    path === 'AGENTS.md' ||
    path === 'scripts/ci.mjs' ||
    /^\.github\/workflows\/[A-Za-z0-9_-]+\.ya?ml$/.test(path) ||
    /^scripts\/ci-(?:scope|gate)(?:\.test)?\.mjs$/.test(path)
  )
    return 'automation'
  const parts = path.split('/')
  if (parts.some((part) => !part || part === '.' || part === '..' || ruleName.test(part)))
    return 'full'
  if (['README.md', 'CHANGELOG.md', 'LICENSE'].includes(path)) return 'docs'
  if (path.startsWith('docs/') && path.endsWith('.md')) return 'docs'
  if (path.startsWith('packages/ui/src/') && /\.(?:ts|svelte|css)$/.test(path)) return 'frontend'
  return 'full'
}

export function classifyPaths(paths, versionBumps = new Set()) {
  if (!Array.isArray(paths) || paths.length === 0) return 'full'
  let scope = 'docs'
  for (const path of paths) {
    const category = versionBumps.has(path) ? 'docs' : pathScope(path)
    if (category === 'full') return 'full'
    if (category === 'frontend' || (category === 'automation' && scope === 'docs')) scope = category
  }
  return scope
}

function detectScope() {
  const fallback = {
    scope: 'full',
    counts: null,
    reason: 'Event or checkout could not be verified.',
  }
  try {
    const eventName = process.env.GITHUB_EVENT_NAME
    if (eventName !== 'push' && eventName !== 'pull_request') return fallback
    const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'))
    const base = eventName === 'push' ? event?.before : event?.pull_request?.base?.sha
    const head = eventName === 'push' ? event?.after : event?.pull_request?.head?.sha
    if (
      typeof base !== 'string' ||
      typeof head !== 'string' ||
      !shaPattern.test(base) ||
      !shaPattern.test(head)
    )
      return fallback

    const git = (args) =>
      execFileSync('git', args, {
        cwd: process.cwd(),
        stdio: ['ignore', 'pipe', 'pipe'],
        maxBuffer: 16 * 1024 * 1024,
      })
    const text = (args) => git(args).toString('utf8').trim()
    const baseCommit = text(['rev-parse', '--verify', `${base}^{commit}`])
    const headCommit = text(['rev-parse', '--verify', `${head}^{commit}`])
    if (baseCommit !== base.toLowerCase() || headCommit !== head.toLowerCase()) return fallback
    const checkout = text(['rev-parse', '--verify', 'HEAD^{commit}'])
    let diffBase = baseCommit
    if (eventName === 'push') {
      if (checkout !== headCommit) return fallback
    } else {
      // A head checkout is exact; a synthetic merge must contain both event commits.
      if (checkout !== headCommit) {
        const parents = text(['rev-list', '--parents', '-n', '1', checkout]).split(' ')
        if (parents.length < 3) return fallback
        git(['merge-base', '--is-ancestor', headCommit, checkout])
        git(['merge-base', '--is-ancestor', baseCommit, checkout])
      }
      const mergeBases = text(['merge-base', '--all', baseCommit, checkout]).split('\n')
      if (mergeBases.length !== 1 || !shaPattern.test(mergeBases[0])) return fallback
      diffBase = mergeBases[0]
    }

    // Disabling rename detection keeps both the removed source and added destination.
    const bytes = git([
      'diff',
      '--no-ext-diff',
      '--no-textconv',
      '--name-only',
      '-z',
      '--no-renames',
      diffBase,
      checkout,
      '--',
    ])
    const names = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
    if (names && !names.endsWith('\0')) return fallback
    const paths = names ? names.slice(0, -1).split('\0') : []
    const versionBumps = detectVersionBumps(
      paths.filter((path) => pathScope(path) === 'full' && versionManifest(path)),
      { text, diffBase, checkout },
    )
    const counts = { docs: 0, automation: 0, frontend: 0, full: 0, version: versionBumps.size }
    for (const path of paths) if (!versionBumps.has(path)) counts[pathScope(path)] += 1
    return {
      scope: classifyPaths(paths, versionBumps),
      counts,
      reason: paths.length
        ? 'Classified all changed paths.'
        : 'No changed paths; full checks are required.',
    }
  } catch {
    return fallback
  }
}

function detectVersionBumps(manifests, { text, diffBase, checkout }) {
  const bumps = new Set()
  const show = (commit, path) => text(['show', `${commit}:${path}`])
  const changesOf = {}
  for (const path of manifests) {
    const patch = text([
      'diff',
      '--no-ext-diff',
      '--no-textconv',
      '--no-renames',
      '-U0',
      diffBase,
      checkout,
      '--',
      path,
    ])
    const changes = replacedLines(`${patch}\n`)
    if (!changes?.length) continue
    const head = show(checkout, path)
    const bumped =
      path === 'Cargo.lock'
        ? lockBumpOnly(show(diffBase, path), head, changes)
        : manifestBumpOnly(path, head, changes)
    if (bumped) {
      bumps.add(path)
      changesOf[path] = changes
    }
  }
  const cargo = ['Cargo.toml', 'Cargo.lock']
  if (cargo.some((path) => bumps.has(path))) {
    const workspace = changesOf['Cargo.toml']
    const follows =
      workspace?.length === 1 &&
      bumps.has('Cargo.lock') &&
      lockFollowsWorkspace(
        show(diffBase, 'Cargo.lock'),
        show(checkout, 'Cargo.lock'),
        cargoVersion.exec(workspace[0][1])[1],
        cargoVersion.exec(workspace[0][2])[1],
      )
    if (!follows) for (const path of cargo) bumps.delete(path)
  }
  return bumps
}

function main() {
  const { scope, counts, reason } = detectScope()
  const output = `scope=${scope}\n`
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, output)
  process.stdout.write(output)
  // Never echo event data or filenames into a workflow command or Markdown summary.
  const categories = counts
    ? `docs=${counts.docs}, frontend=${counts.frontend}, full=${counts.full}, automation=${counts.automation}, version=${counts.version}`
    : 'Path categories unavailable.'
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `## CI scope\n\nScope: **${scope}**\n\n${categories}\n\n${reason}\n`,
    )
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main()
