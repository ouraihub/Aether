import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { extname } from 'node:path'

const limits = new Map([
  ['.cs', 120],
  ['.fs', 120],
  ['.fsx', 120],
  ['.js', 100],
  ['.mjs', 100],
  ['.py', 88],
  ['.ps1', 100],
  ['.psm1', 100],
  ['.rs', 100],
  ['.sh', 100],
  ['.svelte', 100],
  ['.ts', 100],
  ['.tsx', 100],
  ['.vb', 120],
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
  /(^|\/)docs\//,
  /(^|\/)skills\//,
  /(^|\/)(tests?|__tests__)\//,
  /(^|\/)tests?\.[^.]+$/,
]

const isIgnored = (file) => ignored.some((pattern) => pattern.test(file))
const isLongToken = (line) => !/\s/.test(line.trim())
const isComment = (line) => /^(\s*\/\/|\s*#|\s*\/\*|\s*\*|\s*<!--)/.test(line)
const isRegexLiteral = (line) => /^\s*\/.*\/[a-z]*[,;]?$/.test(line)
const hasUrl = (line) => /https?:\/\//.test(line)
const codeWithoutLiterals = (line) => line.replace(/(['"`])(?:\\.|(?!\1).)*\1/g, '')
const literalLength = (line) => line.length - codeWithoutLiterals(line).length
const isMostlyLiteral = (line, limit) => {
  const codeLength = codeWithoutLiterals(line).length
  return codeLength <= limit || literalLength(line) >= line.length / 2
}

const files = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard'], {
  encoding: 'utf8',
})
  .split(/\r?\n/)
  .filter(Boolean)
  .filter((file) => limits.has(extname(file).toLowerCase()))
  .filter((file) => !isIgnored(file))

const violations = []
for (const file of files) {
  const limit = limits.get(extname(file).toLowerCase())
  const lines = readFileSync(file, 'utf8').split(/\r?\n/)
  lines.forEach((line, index) => {
    const length = [...line].length
    if (length <= limit || line.includes('line-length-ignore')) return
    if (
      hasUrl(line) ||
      isLongToken(line) ||
      isComment(line) ||
      isRegexLiteral(line) ||
      isMostlyLiteral(line, limit)
    )
      return
    violations.push(`${file}:${index + 1}: ${length} > ${limit}`)
  })
}

if (violations.length) {
  console.error(`line-length: ${violations.length} violation(s)`)
  console.error(violations.join('\n'))
  process.exitCode = 1
} else {
  console.log(`line-length: ${files.length} production file(s) passed`)
}
