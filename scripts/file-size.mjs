import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { extname } from 'node:path'

const limit = 500
const extensions = new Set([
  '.cs',
  '.fs',
  '.fsx',
  '.js',
  '.mjs',
  '.py',
  '.ps1',
  '.psm1',
  '.rs',
  '.sh',
  '.svelte',
  '.ts',
  '.tsx',
  '.vb',
])
const ignored = [
  /(^|\/)node_modules\//,
  /(^|\/)target\//,
  /(^|\/)dist\//,
  /(^|\/)generated\//,
  /(^|\/)vendor\//,
  /(^|\/)examples\//,
  /(^|\/)plugin\//,
  /(^|\/)crates\//,
  /(^|\/)packages\/contracts\//,
  /(^|\/)packages\/ui\//,
  /(^|\/)packages\/worker\/scenarios\//,
  /(^|\/)tests?\.[^.]+$/,
  /\.(test|spec)\.[^.]+$/,
]
const files = execFileSync(
  'git',
  ['diff', '--name-only', '--diff-filter=A', 'origin/main...HEAD'],
  { encoding: 'utf8' },
)
  .split(/\r?\n/)
  .filter(Boolean)
  .filter((file) => extensions.has(extname(file).toLowerCase()))
  .filter((file) => !ignored.some((pattern) => pattern.test(file)))
const violations = files.flatMap((file) => {
  const lines = readFileSync(file, 'utf8').split(/\r?\n/)
  return lines.length > limit ? [`${file}: ${lines.length} > ${limit}`] : []
})
if (violations.length) {
  console.error(`file-size: ${violations.length} violation(s)`)
  console.error(violations.join('\n'))
  process.exitCode = 1
} else {
  console.log(`file-size: ${files.length} production file(s) passed`)
}
