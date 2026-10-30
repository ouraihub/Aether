// Ratchet on direct clock and randomness reads in production code (not tests).
// Time and ids that drive decisions must come from an injected port (`now`/`newId` options,
// `aether_support::clock::Clock`) so tests can fix them. The baseline lists the boundary
// implementations that are allowed to read the system directly; counts must match exactly, so a
// new direct read fails here and a removed one requires lowering the baseline in the same change.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const ROOTS = [
  {
    dir: 'packages/core/src',
    ext: '.ts',
    pattern: /Date\.now\(|crypto\.randomUUID\(|Math\.random\(|crypto\.getRandomValues\(/g,
  },
  {
    dir: 'packages/worker/src',
    ext: '.ts',
    pattern: /Date\.now\(|crypto\.randomUUID\(|Math\.random\(|crypto\.getRandomValues\(/g,
  },
  {
    dir: 'packages/contracts/src',
    ext: '.ts',
    pattern: /Date\.now\(|crypto\.randomUUID\(|Math\.random\(|crypto\.getRandomValues\(/g,
  },
  { dir: 'crates', ext: '.rs', pattern: /SystemTime::now|getrandom::/g },
  { dir: 'plugin', ext: '.rs', pattern: /SystemTime::now|getrandom::/g },
]
const BASELINE = 'scripts/nondet-baseline.json'
const isTest = (path) => /\.test\.ts$|(^|\/)tests?\/|(^|\/)tests\.rs$|_tests\.rs$|(^|\/)target\//.test(path)

const counts = {}
for (const { dir, ext, pattern } of ROOTS)
  for (const entry of readdirSync(dir, { recursive: true })) {
    const path = join(dir, entry).replaceAll('\\', '/')
    if (!path.endsWith(ext) || isTest(path)) continue
    const found = readFileSync(path, 'utf8').match(pattern)?.length ?? 0
    if (found) counts[path] = found
  }

if (process.argv.includes('--write')) {
  const sorted = Object.fromEntries(Object.entries(counts).sort(([a], [b]) => a.localeCompare(b)))
  writeFileSync(BASELINE, `${JSON.stringify(sorted, null, 2)}\n`)
  process.stdout.write(`nondet-check: wrote ${Object.keys(sorted).length} files to ${BASELINE}\n`)
  process.exit(0)
}

const baseline = JSON.parse(readFileSync(BASELINE, 'utf8'))
const problems = []
for (const path of new Set([...Object.keys(counts), ...Object.keys(baseline)])) {
  const actual = counts[path] ?? 0,
    expected = baseline[path] ?? 0
  if (actual > expected)
    problems.push(
      `${path}: ${actual} direct clock/random reads, baseline ${expected} — inject now/newId (or Clock) instead`,
    )
  else if (actual < expected)
    problems.push(
      `${path}: ${actual} direct reads, baseline ${expected} — lower the baseline (node scripts/nondet-check.mjs --write)`,
    )
}
if (problems.length) {
  for (const line of problems) process.stderr.write(`${line}\n`)
  process.exit(1)
}
process.stdout.write(`nondet-check: ${Object.keys(counts).length} boundary files match the baseline\n`)
