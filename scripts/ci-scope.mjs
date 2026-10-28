// Conservative CI routing. Unrecognized paths, events or checkout relationships run every check.
import { execFileSync } from 'node:child_process'
import { appendFileSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const shaPattern = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i
const ruleName =
  /(?:^|[._-])(?:agents?|claude|skills?|gemini|copilot|instructions?|prompts?|rules?|cursorrules|windsurfrules)(?:[._-]|$)/i

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
  if (parts.some((part) => !part || part === '.' || part === '..' || ruleName.test(part))) return 'full'
  if (['README.md', 'CHANGELOG.md', 'LICENSE'].includes(path)) return 'docs'
  if (path.startsWith('docs/') && path.endsWith('.md')) return 'docs'
  if (path.startsWith('packages/ui/src/') && /\.(?:ts|svelte|css)$/.test(path)) return 'frontend'
  return 'full'
}

export function classifyPaths(paths) {
  if (!Array.isArray(paths) || paths.length === 0) return 'full'
  let scope = 'docs'
  for (const path of paths) {
    const category = pathScope(path)
    if (category === 'full') return 'full'
    if (category === 'frontend' || (category === 'automation' && scope === 'docs')) scope = category
  }
  return scope
}

function detectScope() {
  const fallback = { scope: 'full', counts: null, reason: 'Event or checkout could not be verified.' }
  try {
    const eventName = process.env.GITHUB_EVENT_NAME
    if (eventName !== 'push' && eventName !== 'pull_request') return fallback
    const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8'))
    const base = eventName === 'push' ? event?.before : event?.pull_request?.base?.sha
    const head = eventName === 'push' ? event?.after : event?.pull_request?.head?.sha
    if (typeof base !== 'string' || typeof head !== 'string' || !shaPattern.test(base) || !shaPattern.test(head))
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
    const counts = { docs: 0, automation: 0, frontend: 0, full: 0 }
    for (const path of paths) counts[pathScope(path)] += 1
    return {
      scope: classifyPaths(paths),
      counts,
      reason: paths.length ? 'Classified all changed paths.' : 'No changed paths; full checks are required.',
    }
  } catch {
    return fallback
  }
}

function main() {
  const { scope, counts, reason } = detectScope()
  const output = `scope=${scope}\n`
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, output)
  process.stdout.write(output)
  // Never echo event data or filenames into a workflow command or Markdown summary.
  const categories = counts
    ? `docs=${counts.docs}, frontend=${counts.frontend}, full=${counts.full}, automation=${counts.automation}`
    : 'Path categories unavailable.'
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `## CI scope\n\nScope: **${scope}**\n\n${categories}\n\n${reason}\n`,
    )
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main()
