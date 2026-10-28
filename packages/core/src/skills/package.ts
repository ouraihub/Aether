import type { SkillFileKind } from '@ouraihub/aether-contracts'
import { badRequest, KitFailure } from '../error.js'
import { sha256 } from './content.js'
import { checkSecrets, parseSkillMarkdown, parseSkillSource, type ParsedSkill } from './validation.js'

export interface PackageFile {
  readonly path: string
  readonly kind: SkillFileKind
  readonly sha256: string
  readonly bytes: readonly number[]
}
export interface SkillPackage extends ParsedSkill {
  readonly files: readonly PackageFile[]
}
interface Entry {
  path: string
  offset: number
  compressed: number
  size: number
  method: number
  crc: number
  flags: number
  directory: boolean
}
const TOTAL = 16 * 1024 * 1024

function decode(bytes: Uint8Array, path: string): string {
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes)
  } catch {
    throw badRequest(path, 'not_utf8')
  }
}
function normalize(path: string): string {
  if (path.includes('\\')) throw badRequest('zip', 'backslash_path')
  if (path.startsWith('/') || path.includes(':')) throw badRequest('zip', 'absolute_path')
  const parts = path.split('/').filter((part) => part !== '' && part !== '.')
  if (parts.includes('..')) throw badRequest('zip', 'path_traversal')
  return parts.join('/')
}
function kind(path: string): SkillFileKind | null {
  const top = path.split('/')[0]
  return top === 'scripts' ? 'script' : top === 'references' ? 'reference' : top === 'assets' ? 'asset' : null
}
function checkPath(path: string): void {
  if (!path.split('/').every((part) => /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/u.test(part)))
    throw badRequest(path, 'invalid_name')
  if (path.split('/').length - 1 > 4) throw badRequest(path, 'too_deep', 4)
}
function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff
  for (const byte of bytes) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  return (crc ^ 0xffffffff) >>> 0
}
function entries(bytes: Uint8Array): Entry[] {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  let end = -1
  for (let offset = bytes.length - 22; offset >= Math.max(0, bytes.length - 65557); offset--) {
    if (
      view.getUint32(offset, true) === 0x06054b50 &&
      offset + 22 + view.getUint16(offset + 20, true) === bytes.length
    ) {
      end = offset
      break
    }
  }
  if (end < 0) throw badRequest('zip', 'invalid')
  const count = view.getUint16(end + 10, true)
  if (count > 200) throw badRequest('entries', 'too_large', 200)
  if (
    view.getUint16(end + 4, true) !== 0 ||
    view.getUint16(end + 6, true) !== 0 ||
    view.getUint16(end + 8, true) !== count
  )
    throw badRequest('zip', 'invalid')
  let offset = view.getUint32(end + 16, true)
  const centralEnd = offset + view.getUint32(end + 12, true)
  if (centralEnd !== end) throw badRequest('zip', 'invalid')
  const result: Entry[] = []
  const paths = new Set<string>()
  for (let index = 0; index < count; index++) {
    if (offset + 46 > centralEnd || view.getUint32(offset, true) !== 0x02014b50) throw badRequest('zip', 'invalid')
    const flags = view.getUint16(offset + 8, true)
    if ((flags & 1) !== 0 || (flags & 64) !== 0) throw badRequest('zip', 'encrypted')
    const mode = view.getUint32(offset + 38, true) >>> 16
    if ((mode & 0xf000) === 0xa000) throw badRequest('zip', 'link')
    const nameLength = view.getUint16(offset + 28, true)
    const extraLength = view.getUint16(offset + 30, true)
    const commentLength = view.getUint16(offset + 32, true)
    const next = offset + 46 + nameLength + extraLength + commentLength
    if (next > centralEnd || view.getUint16(offset + 34, true) !== 0) throw badRequest('zip', 'invalid')
    const rawPath = decode(bytes.subarray(offset + 46, offset + 46 + nameLength), 'zip')
    const path = normalize(rawPath)
    if (paths.has(path)) throw badRequest('zip', 'duplicate_path')
    paths.add(path)
    const method = view.getUint16(offset + 10, true)
    if (method !== 0 && method !== 8) throw badRequest('zip', 'invalid')
    const local = view.getUint32(offset + 42, true)
    const compressed = view.getUint32(offset + 20, true)
    if (
      local + 30 > view.byteLength ||
      view.getUint32(local, true) !== 0x04034b50 ||
      view.getUint16(local + 6, true) !== flags ||
      view.getUint16(local + 8, true) !== method
    )
      throw badRequest('zip', 'invalid')
    const localNameLength = view.getUint16(local + 26, true)
    const start = local + 30 + localNameLength + view.getUint16(local + 28, true)
    if (
      start + compressed > view.getUint32(end + 16, true) ||
      decode(bytes.subarray(local + 30, local + 30 + localNameLength), 'zip') !== rawPath
    )
      throw badRequest('zip', 'invalid')
    result.push({
      path,
      directory: rawPath.endsWith('/'),
      offset: start,
      compressed,
      size: view.getUint32(offset + 24, true),
      crc: view.getUint32(offset + 16, true),
      flags,
      method,
    })
    offset = next
  }
  if (offset !== centralEnd) throw badRequest('zip', 'invalid')
  return result
}
async function inflate(
  bytes: Uint8Array,
  entry: Entry,
  limit: number,
  field: string,
): Promise<Uint8Array<ArrayBuffer>> {
  const compressed = bytes.slice(entry.offset, entry.offset + entry.compressed)
  if (entry.method === 0) {
    if (compressed.length > limit) throw badRequest(field, 'too_large', limit)
    return compressed
  }
  const stream = new Blob([compressed]).stream().pipeThrough(new DecompressionStream('deflate-raw'))
  const reader = stream.getReader()
  const chunks: Uint8Array[] = []
  let size = 0
  try {
    while (true) {
      const chunk = await reader.read()
      if (chunk.done) break
      size += chunk.value.length
      if (size > limit) {
        await reader.cancel()
        throw badRequest(field, 'too_large', limit)
      }
      chunks.push(chunk.value)
    }
  } finally {
    reader.releaseLock()
  }
  const result = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    result.set(chunk, offset)
    offset += chunk.length
  }
  return result
}

/** Bounded ZIP/SKILL.md byte import; never reads paths, writes files or executes scripts. */
export async function readSkillSource(value: unknown): Promise<SkillPackage> {
  const source = parseSkillSource(value)
  if (!('bytes' in source)) throw badRequest('source', 'unsupported_source')
  const bytes = new Uint8Array(source.bytes.bytes)
  if (!source.bytes.fileName.toLowerCase().endsWith('.zip')) {
    const raw = decode(bytes, 'SKILL.md')
    checkSecrets(raw, 'SKILL.md')
    return { ...parseSkillMarkdown(raw), files: [] }
  }
  try {
    const allEntries = entries(bytes)
    const fileEntries = allEntries.filter((entry) => !entry.directory)
    const first = fileEntries[0]?.path
    const prefix = first?.includes('/') ? first.slice(0, first.indexOf('/') + 1) : null
    const strip = prefix !== null && fileEntries.every((entry) => entry.path.startsWith(prefix)) ? prefix.length : 0
    const files: PackageFile[] = []
    const unsupported: string[] = []
    let raw: string | undefined
    let license: string | null = null
    let total = 0
    for (const entry of allEntries) {
      const path = entry.path.slice(strip)
      if (entry.directory) {
        if (path && kind(path) === null) unsupported.push(`${path}/`)
        if (path && kind(path) !== null) checkPath(`${path}/directory`)
        continue
      }
      const fileKind = path.includes('/') ? kind(path) : null
      const isSkill = path === 'SKILL.md'
      const isLicense = ['LICENSE', 'LICENSE.md', 'LICENSE.txt'].includes(path)
      if (!isSkill && !isLicense && fileKind === null) {
        unsupported.push(path)
        continue
      }
      if (fileKind !== null) checkPath(path)
      const ownLimit = isSkill ? 65536 : isLicense ? 32768 : 1024 * 1024
      const limit = Math.min(ownLimit, TOTAL - total)
      const field = limit < ownLimit ? 'total' : path
      const content = await inflate(bytes, entry, limit, field)
      if (content.length !== entry.size || crc32(content) !== entry.crc) throw badRequest('zip', 'invalid')
      total += content.length
      let value: string | undefined
      if (fileKind === 'asset') {
        try {
          value = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(content)
        } catch {
          /* Binary assets are permitted. */
        }
      } else value = decode(content, path)
      if (value !== undefined) checkSecrets(value, path)
      if (isSkill) raw = value
      else if (isLicense) {
        if (license !== null) throw badRequest(path, 'duplicate')
        license = value ?? null
      } else if (fileKind !== null)
        files.push({ path, kind: fileKind, bytes: Array.from(content), sha256: await sha256(content) })
    }
    if (unsupported.length) throw badRequest(unsupported.sort().join(', '), 'unsupported_files')
    if (raw === undefined) throw badRequest('SKILL.md', 'missing')
    return {
      ...parseSkillMarkdown(raw),
      license,
      files: files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)),
    }
  } catch (error) {
    if (error instanceof KitFailure) throw error
    throw badRequest('zip', 'invalid')
  }
}
