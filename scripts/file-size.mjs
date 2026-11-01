import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
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
  /(^|\/)(tests?|__tests__|fixtures?|scenarios)\//,
  /(^|\/)(tests?|[\w-]+_tests)\.[^.]+$/,
  /\.(test|spec)\.[^.]+$/,
  // Embedded third-party creator bundle and translation catalogs are resources, not implementation.
  /^crates\/aether-skills\/src\/creator\//,
  /^packages\/ui\/src\/i18n\/(en|zh-CN)\.ts$/,
  // AJV standalone output is generated alongside the public contract bindings.
  /^packages\/contracts\/src\/validators\.js$/,
]
const files = [
  ...new Set(
    execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], {
      encoding: 'utf8',
    })
      .split('\0')
      .filter(Boolean),
  ),
]
  .filter((file) => existsSync(file))
  .filter((file) => extensions.has(extname(file).toLowerCase()))
  .filter((file) => !ignored.some((pattern) => pattern.test(file)))
const violations = files.flatMap((file) => {
  const text = readFileSync(file, 'utf8')
  const lines = text.split(/\r?\n/)
  if (lines.at(-1) === '') lines.pop()
  return lines.length > limit ? [`${file}: ${lines.length} > ${limit}`] : []
})
if (violations.length) {
  console.error(`file-size: ${violations.length} violation(s)`)
  console.error(violations.join('\n'))
  process.exitCode = 1
} else {
  console.log(`file-size: ${files.length} production file(s) passed`)
}
