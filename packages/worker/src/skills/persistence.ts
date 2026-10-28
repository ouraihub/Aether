import type { SkillMeta } from '@ouraihub/aether-contracts'
import { KitFailure } from '@ouraihub/aether-core'
import { digest, sha256 } from '@ouraihub/aether-core/skills/content'
import { parseCatalog, type SkillCatalog, type SkillRecord } from '@ouraihub/aether-core/skills/service'
import { exact, object, revision, text } from '@ouraihub/aether-core/skills/validation'
import type { Actor } from '../context.js'
import type { FileStore } from '../files.js'

export interface SkillBlobRef extends SkillMeta {
  readonly blobId: string
  readonly digest: string
}
export interface LoadedSkills {
  readonly catalog: SkillCatalog
  readonly refs: readonly SkillBlobRef[]
}

export function parseRefs(value: unknown): SkillBlobRef[] {
  if (!Array.isArray(value)) throw new KitFailure('internal', 'Invalid skill catalog')
  const ids = new Set<string>()
  const names = new Set<string>()
  return value.map((raw: unknown) => {
    const entry = object(raw, 'skill')
    exact(
      entry,
      [
        'id',
        'blobId',
        'digest',
        'name',
        'description',
        'version',
        'origin',
        'revision',
        'bodyChars',
        'updatedAt',
        'hasScripts',
        'fileCount',
      ],
      'skill',
    )
    const origin = entry.origin
    if (origin !== 'manual' && origin !== 'imported' && origin !== 'builtin')
      throw new KitFailure('internal', 'Invalid skill origin')
    const { bodyChars, updatedAt, hasScripts, fileCount } = entry
    if (
      typeof bodyChars !== 'number' ||
      !Number.isSafeInteger(bodyChars) ||
      bodyChars < 1 ||
      typeof updatedAt !== 'number' ||
      !Number.isSafeInteger(updatedAt) ||
      typeof hasScripts !== 'boolean' ||
      typeof fileCount !== 'number' ||
      !Number.isInteger(fileCount) ||
      fileCount < 0 ||
      fileCount > 200
    )
      throw new KitFailure('internal', 'Invalid skill metadata')
    const ref: SkillBlobRef = {
      id: text(entry.id, 'id'),
      blobId: text(entry.blobId, 'blobId'),
      digest: text(entry.digest, 'digest'),
      name: text(entry.name, 'name'),
      description: text(entry.description, 'description'),
      version: entry.version === null ? null : text(entry.version, 'version'),
      origin,
      revision: revision(entry.revision),
      bodyChars,
      updatedAt,
      hasScripts,
      fileCount,
    }
    if (
      ids.has(ref.id) ||
      names.has(ref.name) ||
      !/^[A-Za-z0-9_-]{1,128}$/u.test(ref.blobId) ||
      !/^[0-9a-f]{64}$/u.test(ref.digest)
    )
      throw new KitFailure('internal', 'Invalid skill catalog')
    ids.add(ref.id)
    names.add(ref.name)
    return ref
  })
}

function encode(bytes: readonly number[]): string {
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}
function decode(value: unknown): number[] {
  const encoded = text(value, 'file.bytes')
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(encoded))
    throw new KitFailure('internal', 'Invalid skill blob')
  const binary = atob(encoded)
  return Array.from(binary, (char) => char.charCodeAt(0))
}
export function serializeSkill(skill: SkillRecord): Uint8Array {
  return new TextEncoder().encode(
    JSON.stringify({ ...skill, files: skill.files.map((file) => ({ ...file, bytes: encode(file.bytes) })) }),
  )
}
export async function loadSkills(files: FileStore, actor: Actor, value: unknown): Promise<LoadedSkills> {
  const refs = parseRefs(value)
  const catalog: SkillRecord[] = []
  for (const ref of refs) {
    const blob = await files.get(actor, ref.blobId)
    if (!blob || blob.mime !== 'application/json') throw new KitFailure('internal', 'Skill package is unavailable')
    let parsed: unknown
    try {
      parsed = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(blob.data))
    } catch {
      throw new KitFailure('internal', 'Invalid skill package')
    }
    const record = object(parsed, 'skill')
    const storedFiles: unknown = record.files
    if (!Array.isArray(storedFiles)) throw new KitFailure('internal', 'Invalid skill files')
    const decodedFiles = storedFiles.map((value: unknown) => {
      const file = object(value, 'file')
      return { ...file, bytes: decode(file.bytes) }
    })
    const skill = parseCatalog([{ ...record, files: decodedFiles }])[0]
    if (
      !skill ||
      skill.id !== ref.id ||
      skill.digest !== ref.digest ||
      skill.revision !== ref.revision ||
      skill.name !== ref.name ||
      catalog.some((entry) => entry.id === skill.id || entry.name === skill.name)
    )
      throw new KitFailure('internal', 'Invalid skill catalog')
    for (const file of skill.files) {
      if ((await sha256(new Uint8Array(file.bytes))) !== file.sha256)
        throw new KitFailure('internal', 'Skill file integrity failure')
    }
    if ((await digest(skill, skill.files)) !== skill.digest) throw new KitFailure('internal', 'Skill integrity failure')
    catalog.push(skill)
  }
  return { catalog, refs }
}
