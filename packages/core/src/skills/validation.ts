import type { ManualSave, Source } from '@ouraihub/aether-contracts'
import { badRequest } from '../error.js'
import type { ContentIdentity } from './content.js'

export interface ParsedSkill extends ContentIdentity {
  readonly extra: Record<string, string>
}
const encoder = new TextEncoder()

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
export function object(value: unknown, field: string): Record<string, unknown> {
  if (!isObject(value)) throw badRequest(field, 'invalid')
  return value
}
export function exact(value: Record<string, unknown>, keys: readonly string[], field: string): void {
  if (Object.keys(value).some((key) => !keys.includes(key))) throw badRequest(field, 'unknown_field')
}
export function text(value: unknown, field: string): string {
  if (typeof value !== 'string') throw badRequest(field, 'invalid')
  return value
}
export function revision(value: unknown, field = 'revision'): number {
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 1 || value > 0xffffffff)
    throw badRequest(field, 'invalid')
  return value
}
export function strings(value: unknown, field: string): string[] {
  if (!Array.isArray(value)) throw badRequest(field, 'invalid')
  return value.map((entry: unknown) => text(entry, field))
}
export function checkSecrets(value: string, field: string): void {
  const prefixes = ['sk-', 'ghp_', 'gho_', 'github_pat_', 'xoxb-', 'xoxp-', 'AKIA', 'AIza', 'glpat-']
  let lineNumber = 0
  for (const line of value.split('\n')) {
    lineNumber++
    const bearer = /bearer ([A-Za-z0-9_./+=-]+)/i.exec(line)
    const tokens = line.split(/[^A-Za-z0-9_./+=-]/u)
    if (
      (line.includes('-----BEGIN') && line.includes('PRIVATE KEY')) ||
      (bearer?.[1] !== undefined && /^[A-Za-z0-9_./+=-]{20}/u.test(bearer[1])) ||
      tokens.some((token) => token.length >= 20 && prefixes.some((prefix) => token.startsWith(prefix)))
    ) {
      throw badRequest(`${field}:${lineNumber}`, 'secret_detected')
    }
  }
}
export function validateSkillContent(skill: Pick<ContentIdentity, 'name' | 'description' | 'version' | 'body'>): void {
  if (!/^[a-z0-9][a-z0-9-]{0,63}$/u.test(skill.name)) throw badRequest('name', 'invalid')
  if ([...skill.description].length < 1 || [...skill.description].length > 500)
    throw badRequest('description', 'invalid', 500)
  if (skill.version !== null && [...skill.version].length > 32) throw badRequest('version', 'invalid', 32)
  if (!skill.body.trim()) throw badRequest('body', 'empty')
  if (encoder.encode(skill.body).length > 65536) throw badRequest('body', 'too_large', 65536)
  checkSecrets(skill.name, 'name')
  checkSecrets(skill.description, 'description')
  if (skill.version !== null) checkSecrets(skill.version, 'version')
  checkSecrets(skill.body, 'body')
}
export function parseManualSave(value: unknown): ManualSave {
  const input = object(value, 'input')
  exact(input, ['id', 'expectedRevision', 'name', 'description', 'version', 'body'], 'input')
  const result: ManualSave = {
    name: text(input.name, 'name'),
    description: text(input.description, 'description'),
    body: text(input.body, 'body'),
    version: input.version === undefined || input.version === null ? null : text(input.version, 'version'),
    ...(input.id === undefined ? {} : { id: text(input.id, 'id') }),
    ...(input.expectedRevision === undefined
      ? {}
      : { expectedRevision: revision(input.expectedRevision, 'expectedRevision') }),
  }
  validateSkillContent({ ...result, version: result.version ?? null })
  return result
}

function scalar(value: string): string | null {
  if (value.startsWith('"')) {
    let result = ''
    for (let index = 1; index < value.length; index++) {
      const char = value[index]
      if (char === '"') return index === value.length - 1 ? result : null
      if (char === '\\') {
        const next = value[++index]
        if (next !== '"' && next !== '\\') return null
        result += next
      } else result += char
    }
    return null
  }
  if (value.startsWith("'")) {
    if (value.length < 2 || !value.endsWith("'")) return null
    const inner = value.slice(1, -1)
    return inner.includes("'") ? null : inner
  }
  if (
    !value ||
    '[]{},#&*!|>%@`'.includes(value[0] ?? '') ||
    value === '-' ||
    value.startsWith('- ') ||
    value.startsWith('? ') ||
    value.includes(' #') ||
    value.includes(': ')
  )
    return null
  return value
}

export function parseSkillMarkdown(raw: string): ParsedSkill {
  const normalized = raw.replace(/^\uFEFF/u, '').replaceAll('\r\n', '\n')
  if (!normalized.startsWith('---\n')) throw badRequest('1', 'frontmatter_unsupported')
  const fields = new Map<string, string>()
  let offset = 4
  let consumed = 0
  let lineNumber = 1
  let body: string | undefined
  while (offset < normalized.length) {
    lineNumber++
    const newline = normalized.indexOf('\n', offset)
    const end = newline === -1 ? normalized.length : newline
    const line = normalized.slice(offset, end)
    const rawLine = normalized.slice(offset, newline === -1 ? end : end + 1)
    consumed += encoder.encode(rawLine).length
    offset = newline === -1 ? end : end + 1
    if (line === '---') {
      body = normalized.slice(offset)
      break
    }
    if (consumed > 4096) throw badRequest(String(lineNumber), 'frontmatter_unsupported', 4096)
    const separator = line.indexOf(': ')
    const key = line.slice(0, separator)
    const value = separator < 0 ? null : scalar(line.slice(separator + 2).trim())
    if (!/^[a-z][a-z0-9_-]{0,31}$/u.test(key) || value === null || fields.has(key))
      throw badRequest(String(lineNumber), 'frontmatter_unsupported')
    fields.set(key, value)
  }
  if (body === undefined) throw badRequest(String(lineNumber + 1), 'frontmatter_unsupported')
  if (body.split('\n', 1)[0] === '---') throw badRequest(String(lineNumber + 1), 'frontmatter_unsupported')
  const name = fields.get('name')
  const description = fields.get('description')
  if (name === undefined) throw badRequest('name', 'missing')
  if (description === undefined) throw badRequest('description', 'missing')
  const version = fields.get('version') ?? null
  fields.delete('name')
  fields.delete('description')
  fields.delete('version')
  const skill: ParsedSkill = { name, description, version, body, license: null, extra: Object.fromEntries(fields) }
  validateSkillContent(skill)
  return skill
}

/** Filesystem paths are unavailable in the Web runtime. */
export function parseSkillSource(value: unknown): Source {
  const source = object(value, 'source')
  if ('path' in source) throw badRequest('source', 'unsupported_source')
  exact(source, ['bytes'], 'source')
  const input = object(source.bytes, 'bytes')
  exact(input, ['fileName', 'bytes'], 'bytes')
  const fileName = text(input.fileName, 'fileName')
  const isZip = fileName.toLowerCase().endsWith('.zip')
  const bytes: unknown = input.bytes
  if (!Array.isArray(bytes)) throw badRequest('bytes', 'invalid')
  const limit = isZip ? 5 * 1024 * 1024 : 65536
  if (bytes.length > limit) throw badRequest(isZip ? 'zip' : 'SKILL.md', 'too_large', limit)
  const checked = bytes.map((byte: unknown) => {
    if (typeof byte !== 'number' || !Number.isInteger(byte) || byte < 0 || byte > 255)
      throw badRequest('bytes', 'invalid')
    return byte
  })
  return { bytes: { fileName, bytes: checked } }
}
