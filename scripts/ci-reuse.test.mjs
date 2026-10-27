import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { downloadReceipts, recordReceipt, resolveReuse, writeDecision } from './ci-reuse.mjs'

const platforms = ['windows', 'macos-core', 'linux-core']
const sha = '1'.repeat(40)
const base = '2'.repeat(40)
const head = '3'.repeat(40)
const tree = '4'.repeat(40)
const blob = '5'.repeat(40)
const other = '6'.repeat(40)
const repository = 'ouraihub/aether'
const workflow = '.github/workflows/ci.yml'

function fixture() {
  const env = {
    GITHUB_EVENT_NAME: 'push',
    GITHUB_REF: 'refs/heads/main',
    GITHUB_SHA: sha,
    GITHUB_REPOSITORY: repository,
  }
  const gitValues = new Map([
    ['rev-parse HEAD', sha],
    [`show -s --format=%P ${sha}`, `${base} ${head}`],
    [`rev-parse ${sha}^{tree}`, tree],
    [`rev-parse ${sha}:${workflow}`, blob],
    [`rev-parse ${base}:${workflow}`, blob],
  ])
  const pulls = [
    {
      number: 7,
      state: 'closed',
      merged_at: '2026-10-01T00:00:00Z',
      merge_commit_sha: sha,
      base: { ref: 'main', repo: { full_name: repository } },
      head: { sha: head, ref: 'feature', repo: { full_name: repository } },
    },
  ]
  const run = {
    id: 10,
    run_number: 20,
    run_attempt: 2,
    event: 'pull_request',
    head_sha: head,
    head_branch: 'feature',
    path: workflow,
    repository: { full_name: repository },
    head_repository: { full_name: repository },
    pull_requests: [{ number: 7 }],
    status: 'completed',
    conclusion: 'success',
  }
  const runs = { total_count: 1, workflow_runs: [run] }
  const jobs = {
    total_count: 3,
    jobs: platforms.map((name) => ({
      name: { windows: 'build-windows', 'macos-core': 'build-macos', 'linux-core': 'build-linux' }[name],
      run_id: 10,
      run_attempt: 2,
      head_sha: head,
      status: 'completed',
      conclusion: 'success',
    })),
  }
  const artifacts = {
    total_count: 3,
    artifacts: platforms.map((platform, index) => ({
      id: 100 + index,
      name: `ci-receipt-${platform}-2`,
      expired: false,
      expires_at: '2099-01-01T00:00:00Z',
      workflow_run: { id: 10, head_sha: head },
    })),
  }
  const receipts = platforms.map((platform) => ({
    schemaVersion: 1,
    platform,
    repository,
    runId: '10',
    runAttempt: '2',
    tree,
    workflowBlob: blob,
  }))
  const calls = []
  const responses = new Map([
    [`repos/${repository}/commits/${sha}/pulls?per_page=100`, pulls],
    [`repos/${repository}/actions/workflows/ci.yml/runs?event=pull_request&head_sha=${head}&per_page=100`, runs],
    [`repos/${repository}/actions/runs/10/attempts/2/jobs?per_page=100`, jobs],
    [`repos/${repository}/actions/runs/10/artifacts?per_page=100`, artifacts],
  ])
  return {
    env,
    gitValues,
    pulls,
    run,
    runs,
    jobs,
    artifacts,
    receipts,
    calls,
    git(args) {
      assert.ok(gitValues.has(args.join(' ')), `Unexpected git command: ${args}`)
      return gitValues.get(args.join(' '))
    },
    api(path) {
      calls.push(path)
      assert.ok(responses.has(path), `Unexpected API request: ${path}`)
      return responses.get(path)
    },
    readReceipts(identity) {
      assert.deepEqual(identity, { repository, runId: '10', runAttempt: '2' })
      return receipts
    },
  }
}

test('exact tested merge tree and workflow reuse the latest successful three-platform attempt', () => {
  const data = fixture()
  const decision = resolveReuse(data)
  assert.equal(decision.reuse, true)
  assert.deepEqual(decision.provenance, { repository, pr: 7, runId: '10', runAttempt: '2', tree, workflowBlob: blob })
  assert.equal(data.calls.length, 4)
})

for (const [name, mutate, reason] of [
  [
    'pull request',
    (f) => {
      f.env.GITHUB_EVENT_NAME = 'pull_request'
    },
    /Only main push/,
  ],
  [
    'workflow dispatch',
    (f) => {
      f.env.GITHUB_EVENT_NAME = 'workflow_dispatch'
    },
    /Only main push/,
  ],
  [
    'non-main push',
    (f) => {
      f.env.GITHUB_REF = 'refs/heads/topic'
    },
    /Only main push/,
  ],
  [
    'missing repository',
    (f) => {
      delete f.env.GITHUB_REPOSITORY
    },
    /repository/,
  ],
  [
    'invalid pushed SHA',
    (f) => {
      f.env.GITHUB_SHA = '--bad'
    },
    /Checkout/,
  ],
  [
    'wrong checkout',
    (f) => {
      f.gitValues.set('rev-parse HEAD', other)
    },
    /Checkout/,
  ],
  [
    'direct or squash push',
    (f) => {
      f.gitValues.set(`show -s --format=%P ${sha}`, base)
    },
    /two-parent/,
  ],
  [
    'octopus merge',
    (f) => {
      f.gitValues.set(`show -s --format=%P ${sha}`, `${base} ${head} ${other}`)
    },
    /two-parent/,
  ],
  [
    'invalid tree',
    (f) => {
      f.gitValues.set(`rev-parse ${sha}^{tree}`, '')
    },
    /Invalid merge tree/,
  ],
  [
    'first rollout changes workflow',
    (f) => {
      f.gitValues.set(`rev-parse ${base}:${workflow}`, other)
    },
    /changes the CI workflow/,
  ],
  [
    'missing PR',
    (f) => {
      f.pulls.length = 0
    },
    /unique merged PR/,
  ],
  [
    'unmerged PR',
    (f) => {
      f.pulls[0].merged_at = null
    },
    /unique merged PR/,
  ],
  [
    'wrong merge commit',
    (f) => {
      f.pulls[0].merge_commit_sha = other
    },
    /unique merged PR/,
  ],
  [
    'wrong PR base',
    (f) => {
      f.pulls[0].base.ref = 'other'
    },
    /unique merged PR/,
  ],
  [
    'duplicate PR association',
    (f) => {
      f.pulls.push(structuredClone(f.pulls[0]))
    },
    /unique merged PR/,
  ],
  [
    'fork PR',
    (f) => {
      f.pulls[0].head.repo.full_name = 'fork/aether'
    },
    /repository/,
  ],
  [
    'wrong base repository',
    (f) => {
      f.pulls[0].base.repo.full_name = 'fork/aether'
    },
    /repository/,
  ],
  [
    'open PR',
    (f) => {
      f.pulls[0].state = 'open'
    },
    /invalid identity/,
  ],
  [
    'wrong merge parent',
    (f) => {
      f.gitValues.set(`show -s --format=%P ${sha}`, `${base} ${other}`)
    },
    /PR head/,
  ],
  [
    'truncated PR associations',
    (f) => {
      while (f.pulls.length < 100) f.pulls.push(f.pulls[0])
    },
    /truncated/,
  ],
  [
    'missing run',
    (f) => {
      f.runs.total_count = 0
      f.runs.workflow_runs.length = 0
    },
    /No PR workflow/,
  ],
  [
    'truncated runs',
    (f) => {
      f.runs.total_count = 101
    },
    /truncated workflow_runs/,
  ],
  [
    'wrong run repository',
    (f) => {
      f.run.repository.full_name = 'fork/aether'
    },
    /run identity/,
  ],
  [
    'wrong run head repository',
    (f) => {
      f.run.head_repository.full_name = 'fork/aether'
    },
    /run identity/,
  ],
  [
    'wrong run workflow',
    (f) => {
      f.run.path = '.github/workflows/other.yml'
    },
    /run identity/,
  ],
  [
    'wrong run event',
    (f) => {
      f.run.event = 'push'
    },
    /run identity/,
  ],
  [
    'wrong run head',
    (f) => {
      f.run.head_sha = other
    },
    /run identity/,
  ],
  [
    'mismatched run PR association',
    (f) => {
      f.run.pull_requests = [{ number: 8 }]
    },
    /PR association/,
  ],
  [
    'missing run PR list',
    (f) => {
      delete f.run.pull_requests
    },
    /PR association/,
  ],
  [
    'mismatched run branch',
    (f) => {
      f.run.head_branch = 'unrelated'
    },
    /run identity/,
  ],
  [
    'missing PR branch',
    (f) => {
      delete f.pulls[0].head.ref
    },
    /invalid identity/,
  ],
  [
    'invalid run attempt',
    (f) => {
      f.run.run_attempt = 0
    },
    /run identity/,
  ],
  [
    'latest failed run',
    (f) => {
      f.run.conclusion = 'failure'
      f.runs.workflow_runs.push({ ...f.run, id: 9, run_number: 19, conclusion: 'success' })
      f.runs.total_count++
    },
    /Latest PR workflow run/,
  ],
  [
    'latest pending run',
    (f) => {
      f.run.status = 'in_progress'
      f.run.conclusion = null
    },
    /Latest PR workflow run/,
  ],
  [
    'cancelled run',
    (f) => {
      f.run.conclusion = 'cancelled'
    },
    /Latest PR workflow run/,
  ],
  [
    'ambiguous latest run',
    (f) => {
      f.runs.workflow_runs.push({ ...f.run, id: 11 })
      f.runs.total_count++
    },
    /Ambiguous latest/,
  ],
  [
    'truncated jobs',
    (f) => {
      f.jobs.total_count = 101
    },
    /truncated jobs/,
  ],
  [
    'missing platform job',
    (f) => {
      f.jobs.jobs.pop()
      f.jobs.total_count--
    },
    /Missing or duplicate linux-core job/,
  ],
  [
    'duplicate platform job',
    (f) => {
      f.jobs.jobs.push({ ...f.jobs.jobs[0] })
      f.jobs.total_count++
    },
    /duplicate windows job/,
  ],
  [
    'failed platform job',
    (f) => {
      f.jobs.jobs[0].conclusion = 'failure'
    },
    /windows job did not succeed/,
  ],
  [
    'skipped platform job',
    (f) => {
      f.jobs.jobs[0].conclusion = 'skipped'
    },
    /windows job did not succeed/,
  ],
  [
    'pending platform job',
    (f) => {
      f.jobs.jobs[0].status = 'in_progress'
    },
    /windows job did not succeed/,
  ],
  [
    'wrong job run',
    (f) => {
      f.jobs.jobs[0].run_id = 11
    },
    /selected attempt/,
  ],
  [
    'wrong job attempt',
    (f) => {
      f.jobs.jobs[0].run_attempt = 1
    },
    /selected attempt/,
  ],
  [
    'wrong job head',
    (f) => {
      f.jobs.jobs[0].head_sha = other
    },
    /selected attempt/,
  ],
  [
    'truncated artifacts',
    (f) => {
      f.artifacts.total_count = 101
    },
    /truncated artifacts/,
  ],
  [
    'missing artifact',
    (f) => {
      f.artifacts.artifacts.pop()
      f.artifacts.total_count--
    },
    /Missing or duplicate linux-core receipt artifact/,
  ],
  [
    'duplicate artifact',
    (f) => {
      f.artifacts.artifacts.push({ ...f.artifacts.artifacts[0] })
      f.artifacts.total_count++
    },
    /duplicate windows receipt artifact/,
  ],
  [
    'expired artifact',
    (f) => {
      f.artifacts.artifacts[0].expired = true
    },
    /expired/,
  ],
  [
    'stale expiry metadata',
    (f) => {
      f.artifacts.artifacts[0].expires_at = '2000-01-01'
    },
    /expired/,
  ],
  [
    'invalid expiry metadata',
    (f) => {
      delete f.artifacts.artifacts[0].expires_at
    },
    /expired/,
  ],
  [
    'artifact belongs to another run',
    (f) => {
      f.artifacts.artifacts[0].workflow_run.id = 11
    },
    /incorrect identity/,
  ],
  [
    'artifact belongs to another head',
    (f) => {
      f.artifacts.artifacts[0].workflow_run.head_sha = other
    },
    /incorrect identity/,
  ],
  [
    'artifact from old attempt',
    (f) => {
      f.artifacts.artifacts[0].name = 'ci-receipt-windows-1'
    },
    /Missing or duplicate windows receipt artifact/,
  ],
  [
    'missing receipt',
    (f) => {
      f.receipts.pop()
    },
    /Missing receipts/,
  ],
  [
    'duplicate receipt platform',
    (f) => {
      f.receipts[0].platform = 'linux-core'
    },
    /Missing or duplicate windows receipt/,
  ],
  [
    'wrong receipt schema',
    (f) => {
      f.receipts[0].schemaVersion = 2
    },
    /receipt identity/,
  ],
  [
    'wrong receipt repository',
    (f) => {
      f.receipts[0].repository = 'fork/aether'
    },
    /receipt identity/,
  ],
  [
    'wrong receipt run',
    (f) => {
      f.receipts[0].runId = '11'
    },
    /receipt identity/,
  ],
  [
    'wrong receipt attempt',
    (f) => {
      f.receipts[0].runAttempt = '1'
    },
    /receipt identity/,
  ],
  [
    'numeric receipt identity',
    (f) => {
      f.receipts[0].runId = 10
    },
    /receipt identity/,
  ],
  [
    'unexpected receipt fields',
    (f) => {
      f.receipts[0].extra = true
    },
    /receipt identity/,
  ],
  [
    'different tested tree',
    (f) => {
      f.receipts[0].tree = other
    },
    /tree, or workflow/,
  ],
  [
    'different tested workflow',
    (f) => {
      f.receipts[0].workflowBlob = other
    },
    /tree, or workflow/,
  ],
  [
    'git error',
    (f) => {
      f.git = () => {
        throw new Error('missing git evidence')
      }
    },
    /missing git evidence/,
  ],
  [
    'API error',
    (f) => {
      f.api = () => {
        throw new Error('API unavailable')
      }
    },
    /API unavailable/,
  ],
  [
    'malformed API response',
    (f) => {
      f.api = () => null
    },
    /associated PR/,
  ],
  [
    'download or deleted evidence',
    (f) => {
      f.readReceipts = () => {
        throw new Error('artifact deleted')
      }
    },
    /artifact deleted/,
  ],
]) {
  test(`${name} falls back to the full gate with a reason`, () => {
    const data = fixture()
    mutate(data)
    const decision = resolveReuse(data)
    assert.equal(decision.reuse, false)
    assert.match(decision.reason, reason)
    if (['pull request', 'workflow dispatch', 'non-main push', 'first rollout changes workflow'].includes(name)) {
      assert.equal(data.calls.length, 0)
    }
  })
}

test('latest run is chosen by run number, not API ordering or old success', () => {
  const data = fixture()
  data.runs.workflow_runs.unshift({ ...data.run, id: 9, run_number: 19, conclusion: 'failure' })
  data.runs.total_count++
  assert.equal(resolveReuse(data).reuse, true)
})

test('merged PR with an empty API run pull_requests list binds exact head and branch', () => {
  const data = fixture()
  data.run.pull_requests = []
  assert.equal(resolveReuse(data).reuse, true)
  data.run.head_branch = 'unrelated'
  assert.equal(resolveReuse(data).reuse, false)
})

test('a full page is rejected even if total_count claims completeness', () => {
  const data = fixture()
  data.runs.workflow_runs = Array.from({ length: 100 }, () => ({ ...data.run }))
  data.runs.total_count = 100
  assert.match(resolveReuse(data).reason, /truncated workflow_runs/)
})

function temporaryTest(fn) {
  const directory = mkdtempSync(join(tmpdir(), 'aether-reuse-test-'))
  try {
    return fn(directory)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
}

test('record writes a real immutable receipt of the checkout tree, not PR head SHA', () =>
  temporaryTest((directory) => {
    const env = { GITHUB_REPOSITORY: repository, GITHUB_RUN_ID: '10', GITHUB_RUN_ATTEMPT: '2' }
    const git = (args) => {
      if (args.join(' ') === 'rev-parse HEAD^{tree}') return tree
      assert.deepEqual(args, ['rev-parse', `HEAD:${workflow}`])
      return blob
    }
    const receipt = recordReceipt('windows', { env, git, directory })
    assert.deepEqual(JSON.parse(readFileSync(join(directory, 'windows.json'), 'utf8')), receipt)
    assert.equal(receipt.tree, tree)
    assert.equal(receipt.runAttempt, '2')
    assert.throws(() => recordReceipt('windows', { env, git, directory }), /EEXIST/)
    for (const platform of ['invalid', '../windows'])
      assert.throws(() => recordReceipt(platform, { env, git, directory }), /platform/)
    for (const invalid of [
      { ...env, GITHUB_RUN_ID: '' },
      { ...env, GITHUB_RUN_ATTEMPT: '0' },
      { ...env, GITHUB_REPOSITORY: '' },
    ]) {
      assert.throws(() => recordReceipt('linux-core', { env: invalid, git, directory }), /identity/)
    }
    assert.throws(() => recordReceipt('linux-core', { env, git: () => 'invalid', directory }), /tree or workflow/)
  }))

test('download uses argv with fixed artifact names and cleans up all evidence', () =>
  temporaryTest((directory) => {
    const calls = []
    const receipts = fixture().receipts
    const result = downloadReceipts(
      { repository, runId: '10', runAttempt: '2' },
      {
        temporaryRoot: directory,
        exec(args) {
          calls.push(args)
          const index = calls.length - 1
          assert.deepEqual(args.slice(0, 5), ['run', 'download', '10', '-n', `ci-receipt-${platforms[index]}-2`])
          assert.deepEqual(args.slice(-2), ['-R', repository])
          assert.equal(args[5], '-D')
          mkdirSync(args[6])
          writeFileSync(join(args[6], `${platforms[index]}.json`), JSON.stringify(receipts[index]))
        },
      },
    )
    assert.deepEqual(result, receipts)
    assert.deepEqual(readdirSync(directory), [])
  }))

for (const mode of ['download failure', 'bad JSON', 'extra file', 'wrong filename', 'oversized file', 'directory']) {
  test(`${mode} rejects downloaded evidence and still cleans temporary files`, () =>
    temporaryTest((directory) => {
      assert.throws(() =>
        downloadReceipts(
          { repository, runId: '10', runAttempt: '2' },
          {
            temporaryRoot: directory,
            exec(args) {
              mkdirSync(args[6])
              const file = join(args[6], 'windows.json')
              if (mode === 'download failure') throw new Error('download failed')
              if (mode === 'directory') mkdirSync(file)
              else writeFileSync(file, mode === 'oversized file' ? ' '.repeat(16385) : '{bad json')
              if (mode === 'extra file') writeFileSync(join(args[6], 'extra'), '')
              if (mode === 'wrong filename') {
                rmSync(file)
                writeFileSync(join(args[6], 'other.json'), '{}')
              }
            },
          },
        ),
      )
      assert.deepEqual(readdirSync(directory), [])
    }))
}

test(
  'download rejects symbolic link receipts',
  { skip: process.platform === 'win32' ? 'Windows symlink privileges vary' : false },
  () =>
    temporaryTest((directory) => {
      const target = join(directory, 'outside.json')
      writeFileSync(target, '{}')
      assert.throws(
        () =>
          downloadReceipts(
            { repository, runId: '10', runAttempt: '2' },
            {
              temporaryRoot: directory,
              exec(args) {
                mkdirSync(args[6])
                symlinkSync(target, join(args[6], 'windows.json'))
              },
            },
          ),
        /Invalid windows receipt file/,
      )
      assert.deepEqual(readdirSync(directory), ['outside.json'])
    }),
)

test('summary includes exact provenance and output contains the public boolean', () =>
  temporaryTest((directory) => {
    const env = { GITHUB_OUTPUT: join(directory, 'output'), GITHUB_STEP_SUMMARY: join(directory, 'summary') }
    const decision = resolveReuse(fixture())
    assert.equal(writeDecision(decision, { env }).reuse, true)
    assert.equal(readFileSync(env.GITHUB_OUTPUT, 'utf8'), 'reuse=true\n')
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, 'utf8')
    assert.match(summary, /https:\/\/github.com\/ouraihub\/aether\/pull\/7/)
    assert.match(summary, /actions\/runs\/10\/attempts\/2/)
    assert.ok(summary.includes(tree) && summary.includes(blob))
  }))

test('fallback summary explains why all three jobs must run', () =>
  temporaryTest((directory) => {
    const env = { GITHUB_OUTPUT: join(directory, 'output'), GITHUB_STEP_SUMMARY: join(directory, 'summary') }
    writeDecision({ reuse: false, reason: 'Evidence unavailable' }, { env })
    assert.equal(readFileSync(env.GITHUB_OUTPUT, 'utf8'), 'reuse=false\n')
    assert.match(
      readFileSync(env.GITHUB_STEP_SUMMARY, 'utf8'),
      /full three-platform gate.*\nReason: Evidence unavailable/,
    )
  }))

test('summary failure downgrades reuse, but output failure is never swallowed', () => {
  const env = { GITHUB_OUTPUT: 'output', GITHUB_STEP_SUMMARY: 'summary' }
  const writes = []
  const result = writeDecision(resolveReuse(fixture()), {
    env,
    append(path, value) {
      if (path === 'summary') throw new Error('summary unavailable')
      writes.push(value)
    },
    report() {},
  })
  assert.equal(result.reuse, false)
  assert.deepEqual(writes, ['reuse=false\n'])
  assert.throws(
    () =>
      writeDecision(
        { reuse: false, reason: 'Full build' },
        {
          env,
          append() {
            throw new Error('disk full')
          },
          report() {},
        },
      ),
    /disk full/,
  )
  assert.throws(
    () =>
      writeDecision({ reuse: false, reason: 'Full build' }, { env: { GITHUB_STEP_SUMMARY: 'summary' }, append() {} }),
    /GITHUB_OUTPUT/,
  )
})
