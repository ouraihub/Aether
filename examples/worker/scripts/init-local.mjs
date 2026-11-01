import { randomBytes } from 'node:crypto'
import { writeFile } from 'node:fs/promises'

const token = () => randomBytes(32).toString('base64url')
const vars = {
  SMOKE_TOKEN: token(), SMOKE_TOKEN2: token(), MARKER_A: token(), MARKER_B: token(), REAUTH_TOKEN: token(),
  CREDENTIAL_CURRENT_KEY: 'local-1', CREDENTIAL_KEYS: JSON.stringify({ 'local-1': randomBytes(32).toString('base64') }),
}
// Single-quoted dotenv values are literal to wrangler's loader; JSON quoting would leave `\"` in
// CREDENTIAL_KEYS. scripts/smoke.mjs reads the same format.
const line = ([key, value]) => {
  if (/['\r\n]/u.test(value)) throw new Error(`${key} cannot be written as a literal value`)
  return `${key}='${value}'`
}
await writeFile(new URL('../.dev.vars', import.meta.url), Object.entries(vars).map(line).join('\n') + '\n', { flag: 'wx', mode: 0o600 })
console.log('Created local-only .dev.vars. Existing files are never overwritten; keep this file private.')
