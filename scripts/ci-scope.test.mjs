import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { classifyPaths } from './ci-scope.mjs'

const script = fileURLToPath(new URL('./ci-scope.mjs', import.meta.url))

test('classifies ordinary documentation and frontend mixtures conservatively', () => {
  assert.equal(classifyPaths(['README.md', 'CHANGELOG.md', 'LICENSE', 'docs/guide.md', 'docs/nested/setup.md']), 'docs')
  assert.equal(
    classifyPaths(['packages/ui/src/main.ts', 'packages/ui/src/App.svelte', 'packages/ui/src/theme.css']),
    'frontend',
  )
  assert.equal(classifyPaths(['docs/guide.md', 'packages/ui/src/main.ts']), 'frontend')
  assert.equal(classifyPaths([]), 'full')
  assert.equal(classifyPaths(null), 'full')
  assert.equal(classifyPaths(['docs/guide.md', 'Cargo.toml']), 'full')
  assert.equal(classifyPaths(['packages/ui/src/main.ts', '.github/workflows/ci.yml']), 'full')
})

test('rejects rule and instruction documents at every depth and capitalization', () => {
  for (const path of [
    'AGENTS.md',
    'docs/AGENTS.md',
    'docs/nested/CLAUDE.md',
    'docs/SKILL.md',
    'docs/agents.MD',
    'docs/claude.md',
    'docs/GEMINI.md',
    'docs/copilot-instructions.md',
    'docs/instructions/setup.md',
    'docs/prompt.md',
    'docs/rules.md',
    'docs/.cursorrules.md',
    'docs/.windsurfrules.md',
    'packages/ui/src/AGENTS.md',
  ])
    assert.equal(classifyPaths([path]), 'full', path)
})

test('rejects generated DTOs, build inputs and anything outside the allowlist', () => {
  for (const path of [
    'packages/contracts/src/generated/Dto.ts',
    'packages/contracts/src/generated/nested/Dto.ts',
    'packages/contracts/src/generated/README.md',
    'packages/contracts/src/generated',
    'packages/contracts/src/validation.ts',
    'packages/core/src/engine/index.ts',
    'packages/worker/src/app.ts',
    'packages/ui/src/main.js',
    'packages/ui/src/theme.scss',
    'packages/ui/src/icon.svg',
    'packages/ui/src/config.json',
    'packages/ui/package.json',
    'packages/ui/vite.config.ts',
    'packages/ui/tests/check.ts',
    'packages/ui/src-other/main.ts',
    'examples/app/ui/src/main.ts',
    'docs/image.png',
    'docs/config.yml',
    'README.MD',
    'nested/README.md',
    'LICENSE.md',
    'package.json',
    'pnpm-lock.yaml',
    'scripts/ci-scope.mjs',
    '.cargo/config.toml',
    'src/lib.rs',
    'docs/../src/main.md',
    '/docs/setup.md',
    'docs//setup.md',
    './README.md',
    'docs\\setup.md',
    'docs/setup.md\0other',
    '',
    null,
  ])
    assert.equal(classifyPaths([path]), 'full', String(path))
})

function repository(t, objectFormat) {
  const root = mkdtempSync(join(tmpdir(), 'aether-ci-scope-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const cwd = join(root, 'repo')
  mkdirSync(cwd)
  const git = (...args) =>
    execFileSync('git', args, {
      cwd,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      env: {
        ...process.env,
        GIT_AUTHOR_NAME: 'CI test',
        GIT_AUTHOR_EMAIL: 'ci@example.invalid',
        GIT_COMMITTER_NAME: 'CI test',
        GIT_COMMITTER_EMAIL: 'ci@example.invalid',
        GIT_CONFIG_NOSYSTEM: '1',
      },
    }).trim()
  git('init', '-q', ...(objectFormat ? [`--object-format=${objectFormat}`] : []))
  git('config', 'core.autocrlf', 'false')
  git('config', 'commit.gpgsign', 'false')
  const put = (path, content = 'content\n') => {
    const destination = join(cwd, path)
    mkdirSync(dirname(destination), { recursive: true })
    writeFileSync(destination, content)
  }
  const commit = (paths = {}) => {
    for (const [path, content] of Object.entries(paths)) {
      if (content === null) rmSync(join(cwd, path))
      else put(path, content)
    }
    git('add', '--all')
    git('commit', '-qm', 'fixture')
    return git('rev-parse', 'HEAD')
  }
  const run = (eventName, event, { raw = false } = {}) => {
    const eventPath = join(root, 'event.json')
    const outputPath = join(root, 'output')
    const summaryPath = join(root, 'summary')
    writeFileSync(eventPath, raw ? event : JSON.stringify(event))
    writeFileSync(outputPath, 'existing=value\n')
    writeFileSync(summaryPath, '')
    const stdout = execFileSync(process.execPath, [script], {
      cwd,
      encoding: 'utf8',
      env: {
        ...process.env,
        GITHUB_EVENT_NAME: eventName,
        GITHUB_EVENT_PATH: eventPath,
        GITHUB_OUTPUT: outputPath,
        GITHUB_STEP_SUMMARY: summaryPath,
      },
    })
    const output = readFileSync(outputPath, 'utf8')
    assert.match(output, /^existing=value\nscope=(docs|frontend|full)\n$/)
    assert.equal(stdout, output.slice('existing=value\n'.length))
    return { scope: stdout.trim().slice('scope='.length), summary: readFileSync(summaryPath, 'utf8') }
  }
  return { root, cwd, git, put, commit, run }
}

test('push binds the actual checkout and summarizes categories without filenames', (t) => {
  const repo = repository(t)
  const before = repo.commit({ 'Cargo.toml': 'base\n' })
  const after = repo.commit({ 'docs/guide.md': 'guide\n', 'packages/ui/src/main.ts': 'export {}\n' })
  const result = repo.run('push', { before, after })
  assert.equal(result.scope, 'frontend')
  assert.match(result.summary, /docs=1, frontend=1, full=0/)
  assert.doesNotMatch(result.summary, /guide\.md|main\.ts/)
  repo.git('checkout', '--detach', before)
  assert.equal(repo.run('push', { before, after }).scope, 'full')
})

test('deletions and both sides of a rename stay visible', (t) => {
  const repo = repository(t)
  const before = repo.commit({ 'src/lib.rs': 'same content\n', 'docs/old.md': 'guide\n', 'Cargo.toml': 'base\n' })
  repo.put('docs/new.md', readFileSync(join(repo.cwd, 'src/lib.rs'), 'utf8'))
  rmSync(join(repo.cwd, 'src/lib.rs'))
  const after = repo.commit()
  const renamed = repo.run('push', { before, after })
  assert.equal(renamed.scope, 'full')
  assert.match(renamed.summary, /docs=1, frontend=0, full=1/)
  const deleted = repo.commit({ 'Cargo.toml': null })
  assert.equal(repo.run('push', { before: after, after: deleted }).scope, 'full')
  const removedDocs = repo.commit({ 'docs/old.md': null })
  assert.equal(repo.run('push', { before: deleted, after: removedDocs }).scope, 'docs')
})

test('a rename from frontend into unknown code cannot use frontend checks', (t) => {
  const repo = repository(t)
  const before = repo.commit({ 'packages/ui/src/main.ts': 'export {}\n' })
  mkdirSync(join(repo.cwd, 'scripts'))
  renameSync(join(repo.cwd, 'packages/ui/src/main.ts'), join(repo.cwd, 'scripts/main.ts'))
  const after = repo.commit()
  assert.equal(repo.run('push', { before, after }).scope, 'full')
})

test(
  'NUL-delimited filenames preserve tabs, newlines and output-like text',
  { skip: process.platform === 'win32' },
  (t) => {
    const repo = repository(t)
    const before = repo.commit({ 'README.md': 'base\n' })
    const after = repo.commit({ 'docs/guide\n::warning::scope=docs\t.md': 'guide\n', 'src/unknown\nname.rs': 'code\n' })
    const result = repo.run('push', { before, after })
    assert.equal(result.scope, 'full')
    assert.match(result.summary, /docs=1, frontend=0, full=1/)
    assert.doesNotMatch(result.summary, /warning|unknown|guide/)
  },
)

test('PR head checkouts diff from the merge-base rather than the current base tip', (t) => {
  const repo = repository(t)
  const common = repo.commit({ 'README.md': 'base\n' })
  const head = repo.commit({ 'docs/guide.md': 'feature\n' })
  repo.git('checkout', '--detach', common)
  const base = repo.commit({ 'src/lib.rs': 'base-only change\n' })
  repo.git('checkout', '--detach', head)
  const event = { pull_request: { base: { sha: base }, head: { sha: head } } }
  assert.equal(repo.run('pull_request', event).scope, 'docs')
  repo.git('checkout', '--detach', base)
  assert.equal(repo.run('pull_request', event).scope, 'full')
})

test('PR synthetic merges must contain the event head and base', (t) => {
  const repo = repository(t)
  const common = repo.commit({ 'README.md': 'base\n' })
  const head = repo.commit({ 'docs/guide.md': 'feature\n' })
  repo.git('checkout', '--detach', common)
  const base = repo.commit({ 'src/lib.rs': 'base change\n' })
  repo.git('-c', 'merge.gpgsign=false', 'merge', '--no-ff', '--no-edit', head)
  const merge = repo.git('rev-parse', 'HEAD')
  const event = { pull_request: { base: { sha: base }, head: { sha: head } } }
  assert.equal(repo.run('pull_request', event).scope, 'docs')
  // An ordinary descendant is not a synthetic merge checkout.
  repo.commit({ 'docs/extra.md': 'extra\n' })
  assert.equal(repo.run('pull_request', event).scope, 'full')
  repo.git('checkout', '--detach', merge)
  const unrelated = repo.commit({ 'docs/unrelated.md': 'unrelated\n' })
  repo.git('checkout', '--detach', merge)
  assert.equal(
    repo.run('pull_request', { pull_request: { base: { sha: base }, head: { sha: unrelated } } }).scope,
    'full',
  )
  assert.equal(
    repo.run('pull_request', { pull_request: { base: { sha: unrelated }, head: { sha: head } } }).scope,
    'full',
  )
})

test('unknown events, malformed event shapes and unavailable commits fail closed', (t) => {
  const repo = repository(t)
  const before = repo.commit({ 'README.md': 'base\n' })
  const after = repo.commit({ 'docs/guide.md': 'guide\n' })
  for (const eventName of ['workflow_dispatch', 'pull_request_target', 'schedule', '']) {
    assert.equal(repo.run(eventName, { before, after }).scope, 'full')
  }
  for (const event of [
    null,
    {},
    [],
    { before, after: null },
    { before: 'HEAD~1', after },
    { before, after: after.slice(0, 12) },
    { before: '-'.repeat(40), after },
    { before: '0'.repeat(40), after },
    { before, after: 'f'.repeat(40) },
    { before: {}, after },
    { before, after: `${after}\nscope=docs` },
  ]) {
    assert.equal(repo.run('push', event).scope, 'full')
  }
  assert.equal(repo.run('push', '{broken', { raw: true }).scope, 'full')
  assert.equal(repo.run('pull_request', { pull_request: { base: { sha: before } } }).scope, 'full')
  assert.equal(repo.run('push', { before: after, after }).scope, 'full')
  assert.equal(repo.run('push', { before: before.toUpperCase(), after: after.toUpperCase() }).scope, 'docs')
})

test('supports full SHA-256 commit identities', (t) => {
  const repo = repository(t, 'sha256')
  const before = repo.commit({ 'README.md': 'base\n' })
  const after = repo.commit({ 'docs/guide.md': 'guide\n' })
  assert.equal(before.length, 64)
  assert.equal(repo.run('push', { before, after }).scope, 'docs')
})

test('missing event file or Git repository cannot select light checks', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'aether-ci-scope-missing-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const output = join(root, 'output')
  const event = join(root, 'event.json')
  const invoke = () =>
    execFileSync(process.execPath, [script], {
      cwd: root,
      encoding: 'utf8',
      env: {
        ...process.env,
        GITHUB_EVENT_NAME: 'push',
        GITHUB_EVENT_PATH: event,
        GITHUB_OUTPUT: output,
        GITHUB_STEP_SUMMARY: '',
      },
    })
  assert.equal(existsSync(event), false)
  assert.equal(invoke(), 'scope=full\n')
  writeFileSync(event, JSON.stringify({ before: 'a'.repeat(40), after: 'b'.repeat(40) }))
  assert.equal(invoke(), 'scope=full\n')
})
