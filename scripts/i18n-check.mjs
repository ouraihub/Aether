// pnpm kit:i18n — both locales share one key set, and every literal `t('key')` in packages/ui/src exists (docs/16 §5).
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

const SRC = 'packages/ui/src'
const { en } = await import(pathToFileURL(join(SRC, 'i18n/en.ts')).href)
const { zhCN } = await import(pathToFileURL(join(SRC, 'i18n/zh-CN.ts')).href)

const problems = []
const enKeys = new Set(Object.keys(en))
const zhKeys = new Set(Object.keys(zhCN))
for (const key of enKeys) if (!zhKeys.has(key)) problems.push(`zh-CN missing ${key}`)
for (const key of zhKeys) if (!enKeys.has(key)) problems.push(`en missing ${key}`)

// Dynamic keys (`errors.${code}`) are covered by the Messages type instead.
const USE = /\bt\(\s*['"]([^'"]+)['"]/g
for (const entry of readdirSync(SRC, { recursive: true, withFileTypes: true })) {
  if (!entry.isFile() || !/\.(ts|svelte)$/.test(entry.name) || /\.test\.ts$/.test(entry.name)) continue
  const file = join(entry.parentPath, entry.name)
  for (const [, key] of readFileSync(file, 'utf8').matchAll(USE)) {
    if (!enKeys.has(key)) problems.push(`${file}: unknown key ${key}`)
  }
}

if (problems.length > 0) {
  for (const problem of problems) console.error(problem)
  process.exit(1)
}
console.log(`${enKeys.size} keys in en and zh-CN; all used keys exist`)
