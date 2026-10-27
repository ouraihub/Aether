import { execFileSync } from 'node:child_process'
import {
  appendFileSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const platforms = ['windows', 'macos-core', 'linux-core']
const workflow = '.github/workflows/ci.yml'
const hash = (value) => typeof value === 'string' && /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(value)
const positive = (value) =>
  (typeof value === 'number' || typeof value === 'string') &&
  /^[1-9][0-9]*$/.test(String(value)) &&
  Number.isSafeInteger(Number(value))
const requireEvidence = (condition, reason) => {
  if (!condition) throw new Error(reason)
}
const gitCommand = (args) => execFileSync('git', args, { encoding: 'utf8' }).trim()
const ghCommand = (args) => execFileSync('gh', args, { encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 })
const githubApi = (path) => JSON.parse(ghCommand(['api', path]))

function identity(env) {
  requireEvidence(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(env.GITHUB_REPOSITORY ?? ''), 'Invalid repository identity')
  return env.GITHUB_REPOSITORY
}

function completePage(response, key) {
  requireEvidence(
    Number.isSafeInteger(response?.total_count) &&
      Array.isArray(response[key]) &&
      response.total_count === response[key].length &&
      response[key].length < 100,
    `Missing or truncated ${key} evidence`,
  )
  return response[key]
}

export function recordReceipt(platform, { env = process.env, git = gitCommand, directory = '.ci-receipt' } = {}) {
  requireEvidence(platforms.includes(platform), 'Unknown receipt platform')
  const repository = identity(env)
  requireEvidence(
    typeof env.GITHUB_RUN_ID === 'string' &&
      positive(env.GITHUB_RUN_ID) &&
      typeof env.GITHUB_RUN_ATTEMPT === 'string' &&
      positive(env.GITHUB_RUN_ATTEMPT),
    'Invalid run identity',
  )
  const tree = git(['rev-parse', 'HEAD^{tree}'])
  const workflowBlob = git(['rev-parse', `HEAD:${workflow}`])
  requireEvidence(hash(tree) && hash(workflowBlob), 'Invalid checkout tree or workflow blob')
  const receipt = {
    schemaVersion: 1,
    platform,
    repository,
    runId: env.GITHUB_RUN_ID,
    runAttempt: env.GITHUB_RUN_ATTEMPT,
    tree,
    workflowBlob,
  }
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, `${platform}.json`), `${JSON.stringify(receipt, null, 2)}\n`, { flag: 'wx' })
  return receipt
}

export function downloadReceipts(
  { repository, runId, runAttempt },
  { exec = ghCommand, temporaryRoot = tmpdir() } = {},
) {
  const temporary = mkdtempSync(join(temporaryRoot, 'aether-ci-receipts-'))
  try {
    return platforms.map((platform) => {
      const directory = join(temporary, platform)
      exec(['run', 'download', runId, '-n', `ci-receipt-${platform}-${runAttempt}`, '-D', directory, '-R', repository])
      const entries = readdirSync(directory)
      requireEvidence(
        entries.length === 1 && entries[0] === `${platform}.json`,
        `Unexpected ${platform} artifact contents`,
      )
      const file = join(directory, entries[0])
      const stat = lstatSync(file)
      requireEvidence(stat.isFile() && !stat.isSymbolicLink() && stat.size <= 16384, `Invalid ${platform} receipt file`)
      return JSON.parse(readFileSync(file, 'utf8'))
    })
  } finally {
    rmSync(temporary, { recursive: true, force: true })
  }
}

// All uncertainty is a full-build decision; output writing is deliberately outside this catch.
export function resolveReuse({
  env = process.env,
  git = gitCommand,
  api = githubApi,
  readReceipts = downloadReceipts,
  now = Date.now(),
} = {}) {
  try {
    requireEvidence(
      env.GITHUB_EVENT_NAME === 'push' && env.GITHUB_REF === 'refs/heads/main',
      'Only main push events can reuse PR checks',
    )
    const repository = identity(env)
    const sha = env.GITHUB_SHA
    requireEvidence(hash(sha) && git(['rev-parse', 'HEAD']) === sha, 'Checkout does not match the pushed commit')
    const parents = git(['show', '-s', '--format=%P', sha]).split(' ')
    requireEvidence(parents.length === 2 && parents.every(hash), 'Push is not a normal two-parent merge')
    const tree = git(['rev-parse', `${sha}^{tree}`])
    const workflowBlob = git(['rev-parse', `${sha}:${workflow}`])
    requireEvidence(hash(tree) && hash(workflowBlob), 'Invalid merge tree or workflow blob')
    requireEvidence(
      workflowBlob === git(['rev-parse', `${parents[0]}:${workflow}`]),
      'Merge changes the CI workflow; full build required',
    )
    const root = `repos/${repository}`
    const pulls = api(`${root}/commits/${sha}/pulls?per_page=100`)
    requireEvidence(Array.isArray(pulls) && pulls.length < 100, 'Missing or truncated associated PR evidence')
    const merged = pulls.filter((pr) => pr?.merged_at && pr.merge_commit_sha === sha && pr.base?.ref === 'main')
    requireEvidence(merged.length === 1, 'No unique merged PR for this push')
    const pr = merged[0]
    requireEvidence(
      positive(pr.number) &&
        pr.state === 'closed' &&
        pr.base?.repo?.full_name === repository &&
        pr.head?.repo?.full_name === repository &&
        hash(pr.head?.sha) &&
        typeof pr.head.ref === 'string' &&
        pr.head.ref.length > 0,
      'Merged PR is not from this repository or has invalid identity',
    )
    requireEvidence(parents[1] === pr.head.sha, 'Merge parent does not match the PR head')
    const runs = completePage(
      api(`${root}/actions/workflows/ci.yml/runs?event=pull_request&head_sha=${pr.head.sha}&per_page=100`),
      'workflow_runs',
    )
    requireEvidence(runs.length > 0, 'No PR workflow run found')
    for (const run of runs) {
      requireEvidence(
        positive(run?.id) &&
          positive(run.run_number) &&
          positive(run.run_attempt) &&
          run.event === 'pull_request' &&
          run.head_sha === pr.head.sha &&
          run.path === workflow &&
          run.repository?.full_name === repository &&
          run.head_repository?.full_name === repository &&
          run.head_branch === pr.head.ref &&
          Array.isArray(run.pull_requests) &&
          (run.pull_requests.length === 0 || run.pull_requests.some((entry) => entry?.number === pr.number)),
        'Workflow run identity or PR association does not match',
      )
    }
    runs.sort((a, b) => Number(b.run_number) - Number(a.run_number))
    const run = runs[0]
    requireEvidence(
      !runs.slice(1).some((other) => other.run_number === run.run_number),
      'Ambiguous latest workflow run',
    )
    requireEvidence(
      run.status === 'completed' && run.conclusion === 'success',
      'Latest PR workflow run is not successful and completed',
    )
    const runId = String(run.id)
    const runAttempt = String(run.run_attempt)
    const jobs = completePage(api(`${root}/actions/runs/${runId}/attempts/${runAttempt}/jobs?per_page=100`), 'jobs')
    for (const platform of platforms) {
      const matching = jobs.filter((job) => job?.name === platform)
      requireEvidence(matching.length === 1, `Missing or duplicate ${platform} job`)
      const job = matching[0]
      requireEvidence(
        String(job.run_id) === runId &&
          String(job.run_attempt) === runAttempt &&
          job.head_sha === pr.head.sha &&
          job.status === 'completed' &&
          job.conclusion === 'success',
        `${platform} job did not succeed in the selected attempt`,
      )
    }
    const artifacts = completePage(api(`${root}/actions/runs/${runId}/artifacts?per_page=100`), 'artifacts')
    for (const platform of platforms) {
      const matching = artifacts.filter((artifact) => artifact?.name === `ci-receipt-${platform}-${runAttempt}`)
      requireEvidence(matching.length === 1, `Missing or duplicate ${platform} receipt artifact`)
      const artifact = matching[0]
      requireEvidence(
        positive(artifact.id) &&
          artifact.expired === false &&
          Number.isFinite(Date.parse(artifact.expires_at)) &&
          Date.parse(artifact.expires_at) > now &&
          String(artifact.workflow_run?.id) === runId &&
          artifact.workflow_run?.head_sha === pr.head.sha,
        `${platform} receipt artifact is expired or has incorrect identity`,
      )
    }
    const receipts = readReceipts({ repository, runId, runAttempt })
    requireEvidence(Array.isArray(receipts) && receipts.length === platforms.length, 'Missing receipts')
    for (const platform of platforms) {
      const matching = receipts.filter((receipt) => receipt?.platform === platform)
      requireEvidence(matching.length === 1, `Missing or duplicate ${platform} receipt`)
      const receipt = matching[0]
      requireEvidence(
        Object.keys(receipt).length === 7 &&
          receipt.schemaVersion === 1 &&
          receipt.repository === repository &&
          receipt.runId === runId &&
          receipt.runAttempt === runAttempt &&
          receipt.tree === tree &&
          receipt.workflowBlob === workflowBlob,
        `${platform} receipt identity, tree, or workflow does not match`,
      )
    }
    return {
      reuse: true,
      reason: 'All three native jobs successfully tested this exact tree and workflow',
      provenance: { repository, pr: pr.number, runId, runAttempt, tree, workflowBlob },
    }
  } catch (error) {
    return { reuse: false, reason: error instanceof Error ? error.message : 'Unknown evidence error' }
  }
}

export function writeDecision(decision, { env = process.env, append = appendFileSync, report = console.error } = {}) {
  let result = decision
  try {
    requireEvidence(
      typeof env.GITHUB_STEP_SUMMARY === 'string' && env.GITHUB_STEP_SUMMARY.length > 0,
      'GITHUB_STEP_SUMMARY is missing',
    )
    const detail = decision.provenance
    const reason = decision.reason.replace(/[\r\n]/g, ' ')
    const summary = [
      '## Tested-tree CI reuse',
      decision.reuse ? 'Reusing successful three-platform PR checks.' : 'Running the full three-platform gate.',
      `Reason: ${reason}`,
      ...(detail
        ? [
            `Source: [PR #${detail.pr}](https://github.com/${detail.repository}/pull/${detail.pr}), ` +
              `[run ${detail.runId}, attempt ${detail.runAttempt}](https://github.com/${detail.repository}/actions/runs/${detail.runId}/attempts/${detail.runAttempt}).`,
            `Tree: \`${detail.tree}\`; workflow blob: \`${detail.workflowBlob}\`.`,
          ]
        : []),
      '',
    ].join('\n')
    append(env.GITHUB_STEP_SUMMARY, `${summary}\n`)
  } catch (error) {
    result = { reuse: false, reason: `Cannot write reuse summary: ${error.message}` }
    report(result.reason)
  }
  requireEvidence(typeof env.GITHUB_OUTPUT === 'string' && env.GITHUB_OUTPUT.length > 0, 'GITHUB_OUTPUT is missing')
  append(env.GITHUB_OUTPUT, `reuse=${result.reuse}\n`)
  return result
}

export function runCli(args = process.argv.slice(2), options = {}) {
  if (args[0] === 'record' && args.length === 2) return recordReceipt(args[1], options)
  requireEvidence(
    args.length === 1 && args[0] === 'resolve',
    'Usage: ci-reuse.mjs record windows|macos-core|linux-core OR resolve',
  )
  const result = writeDecision(resolveReuse(options), options)
  console.log(`CI reuse: ${result.reuse}: ${result.reason}`)
  return result
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) runCli()
