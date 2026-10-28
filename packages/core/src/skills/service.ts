import type {
  ManualSave,
  Preview,
  SkillDto,
  SkillFileMeta,
  SkillMeta,
  SkillOrigin,
  SkillRef,
} from '@ouraihub/aether-contracts'
import { badRequest, KitFailure } from '../error.js'
import { digest } from './content.js'
import type { PackageFile, SkillPackage } from './package.js'
import { exact, object, revision, text, validateSkillContent } from './validation.js'

export interface SkillSnapshot {
  readonly id: string
  readonly revision: number
  readonly name: string
  readonly body: string
}
export interface BuiltinSkill {
  readonly name: string
  readonly description: string
  readonly version: string | null
  readonly body: string
}
export interface SkillRecord extends SkillPackage {
  readonly id: string
  readonly origin: SkillOrigin
  readonly revision: number
  readonly digest: string
  readonly updatedAt: number
}
export type SkillCatalog = readonly SkillRecord[]
export const conflict = (reason: string, entity?: string): KitFailure =>
  new KitFailure('conflict', 'Skill conflict', { reason, ...(entity === undefined ? {} : { entity }) })
const missing = (entity: string): KitFailure => new KitFailure('not_found', 'Skill not found', { entity })
export function fileMetas(skill: SkillPackage): SkillFileMeta[] {
  return skill.files.map((file) => ({ path: file.path, kind: file.kind, size: file.bytes.length }))
}
export function metadata(skill: SkillRecord): SkillMeta {
  return {
    id: skill.id,
    name: skill.name,
    description: skill.description,
    version: skill.version,
    origin: skill.origin,
    revision: skill.revision,
    bodyChars: [...skill.body].length,
    updatedAt: skill.updatedAt,
    hasScripts: skill.files.some((file) => file.kind === 'script'),
    fileCount: skill.files.length,
  }
}
export function list(catalog: SkillCatalog): SkillMeta[] {
  return catalog.map(metadata)
}
export function record(catalog: SkillCatalog, id: string): SkillRecord {
  const found = catalog.find((skill) => skill.id === id)
  if (!found) throw missing('skill')
  return found
}
export function get(catalog: SkillCatalog, id: string): SkillDto {
  const skill = record(catalog, id)
  return {
    ...metadata(skill),
    body: skill.body,
    license: skill.license,
    extra: { ...skill.extra },
    files: fileMetas(skill),
  }
}
function pinned(catalog: SkillCatalog, id: string, expected: number): SkillRecord {
  revision(expected)
  const skill = catalog.find((entry) => entry.id === id)
  if (!skill || skill.revision !== expected) throw conflict('skill_changed', id)
  return skill
}
export function files(catalog: SkillCatalog, id: string, expected: number): SkillFileMeta[] {
  return fileMetas(pinned(catalog, id, expected))
}
export function file(catalog: SkillCatalog, id: string, expected: number, path: string): Uint8Array<ArrayBuffer> {
  const found = pinned(catalog, id, expected).files.find((entry) => entry.path === path)
  if (!found) throw missing('skill_file')
  return new Uint8Array(found.bytes)
}
export function snapshot(catalog: SkillCatalog, refs: readonly SkillRef[]): SkillSnapshot[] {
  return refs.map((ref) => {
    const skill = pinned(catalog, ref.id, ref.revision)
    return { id: skill.id, revision: skill.revision, name: skill.name, body: skill.body }
  })
}
function ensureFree(catalog: SkillCatalog, name: string, except?: string): void {
  const found = catalog.find((skill) => skill.name === name && skill.id !== except)
  if (found) throw conflict(found.origin === 'builtin' ? 'builtin_name' : 'exists')
}
function mutable(skill: SkillRecord): void {
  if (skill.origin === 'builtin') throw badRequest('id', 'builtin')
}
async function create(content: SkillPackage, id: string, origin: SkillOrigin, now: number): Promise<SkillRecord> {
  return { ...content, id, origin, revision: 1, digest: await digest(content, content.files), updatedAt: now }
}
async function update(
  previous: SkillRecord,
  content: SkillPackage,
  origin: SkillOrigin,
  now: number,
): Promise<SkillRecord> {
  const nextDigest = await digest(content, content.files)
  const changed = nextDigest !== previous.digest
  if (changed && previous.revision === 0xffffffff) throw conflict('revision_exhausted', previous.id)
  return {
    ...content,
    id: previous.id,
    origin,
    revision: previous.revision + (changed ? 1 : 0),
    digest: nextDigest,
    updatedAt: now,
  }
}
export async function preview(catalog: SkillCatalog, content: SkillPackage): Promise<Preview> {
  const current = catalog.find((skill) => skill.name === content.name)
  if (current?.origin === 'builtin') throw conflict('builtin_name')
  return {
    ...content,
    files: fileMetas(content),
    digest: await digest(content, content.files),
    action: current ? 'replace' : 'install',
    current: current ? { version: current.version, revision: current.revision } : null,
  }
}
export async function install(
  catalog: SkillCatalog,
  content: SkillPackage,
  confirmedDigest: string,
  replace: boolean,
  id: string,
  now: number,
): Promise<SkillCatalog> {
  if ((await digest(content, content.files)) !== confirmedDigest) throw conflict('content_changed')
  const current = catalog.find((skill) => skill.name === content.name)
  if (current?.origin === 'builtin') throw conflict('builtin_name')
  if (current && !replace) throw conflict('exists')
  if (!current) {
    ensureId(catalog, id)
    return [...catalog, await create(content, id, 'imported', now)]
  }
  const next = await update(current, content, 'imported', now)
  return catalog.map((skill) => (skill.id === current.id ? next : skill))
}
function ensureId(catalog: SkillCatalog, id: string): void {
  if (!id || catalog.some((skill) => skill.id === id)) throw conflict('id_exists')
}
export async function save(
  catalog: SkillCatalog,
  input: ManualSave,
  newId: string,
  now: number,
): Promise<SkillCatalog> {
  const fields = { name: input.name, description: input.description, version: input.version ?? null, body: input.body }
  validateSkillContent(fields)
  if (input.id === undefined) {
    ensureFree(catalog, input.name)
    ensureId(catalog, newId)
    return [...catalog, await create({ ...fields, license: null, extra: {}, files: [] }, newId, 'manual', now)]
  }
  if (input.expectedRevision === undefined) throw badRequest('expectedRevision', 'missing')
  const current = record(catalog, input.id)
  mutable(current)
  if (current.revision !== input.expectedRevision) throw conflict('revision_changed', input.id)
  ensureFree(catalog, input.name, input.id)
  const next = await update(
    current,
    { ...fields, license: current.license, extra: current.extra, files: current.files },
    current.origin,
    now,
  )
  return catalog.map((skill) => (skill.id === current.id ? next : skill))
}
export async function duplicate(
  catalog: SkillCatalog,
  id: string,
  newName: string,
  newId: string,
  now: number,
): Promise<SkillCatalog> {
  const current = record(catalog, id)
  validateSkillContent({ ...current, name: newName })
  ensureFree(catalog, newName)
  ensureId(catalog, newId)
  return [...catalog, await create({ ...current, name: newName }, newId, 'manual', now)]
}
export function reorder(catalog: SkillCatalog, ids: readonly string[]): SkillCatalog {
  if (
    ids.length !== catalog.length ||
    new Set(ids).size !== ids.length ||
    ids.some((id) => !catalog.some((skill) => skill.id === id))
  )
    throw badRequest('ids', 'mismatch')
  return ids.map((id) => record(catalog, id))
}
export function remove(catalog: SkillCatalog, id: string): SkillCatalog {
  mutable(record(catalog, id))
  return catalog.filter((skill) => skill.id !== id)
}

/** Host built-ins synchronize by name, without overwriting user-owned skills. */
export async function syncBuiltins(
  catalog: SkillCatalog,
  builtins: readonly BuiltinSkill[],
  now: number,
): Promise<SkillCatalog> {
  const names = new Set<string>()
  for (const builtin of builtins) {
    validateSkillContent(builtin)
    if (names.has(builtin.name)) throw badRequest('name', 'duplicate')
    names.add(builtin.name)
  }
  let result = catalog.filter((skill) => skill.origin !== 'builtin' || names.has(skill.name))
  for (const builtin of builtins) {
    const current = result.find((skill) => skill.name === builtin.name)
    if (current && current.origin !== 'builtin') continue
    const content = { ...builtin, license: null, extra: {}, files: [] }
    if (current) {
      if ((await digest(content)) === current.digest) continue
      const next = await update(current, content, 'builtin', now)
      result = result.map((skill) => (skill.id === current.id ? next : skill))
    } else {
      const id = `builtin:${builtin.name}`
      ensureId(result, id)
      result = [...result, await create(content, id, 'builtin', now)]
    }
  }
  return result.length === catalog.length && result.every((skill, index) => skill === catalog[index]) ? catalog : result
}

/** Strictly narrows persisted documents before exposing their contents to services. */
export function parseCatalog(value: unknown): SkillCatalog {
  if (!Array.isArray(value)) throw badRequest('skills', 'invalid_document')
  const ids = new Set<string>()
  const names = new Set<string>()
  return value.map((raw: unknown) => {
    const entry = object(raw, 'skill')
    exact(
      entry,
      [
        'id',
        'name',
        'description',
        'version',
        'body',
        'license',
        'extra',
        'files',
        'origin',
        'revision',
        'digest',
        'updatedAt',
      ],
      'skill',
    )
    const origin = entry.origin
    if (origin !== 'builtin' && origin !== 'manual' && origin !== 'imported') throw badRequest('origin', 'invalid')
    const extras = object(entry.extra, 'extra')
    const extra = Object.fromEntries(Object.entries(extras).map(([key, value]) => [key, text(value, 'extra')]))
    const fileValues: unknown = entry.files
    if (!Array.isArray(fileValues)) throw badRequest('files', 'invalid')
    const parsedFiles: PackageFile[] = fileValues.map((rawFile: unknown) => {
      const file = object(rawFile, 'file')
      exact(file, ['path', 'kind', 'sha256', 'bytes'], 'file')
      const kind = file.kind
      if (kind !== 'script' && kind !== 'reference' && kind !== 'asset') throw badRequest('kind', 'invalid')
      const bytes: unknown = file.bytes
      if (!Array.isArray(bytes)) throw badRequest('bytes', 'invalid')
      const content = bytes.map((byte: unknown) => {
        if (typeof byte !== 'number' || !Number.isInteger(byte) || byte < 0 || byte > 255)
          throw badRequest('bytes', 'invalid')
        return byte
      })
      const hash = text(file.sha256, 'sha256')
      if (!/^[0-9a-f]{64}$/u.test(hash)) throw badRequest('sha256', 'invalid')
      return { path: text(file.path, 'path'), kind, sha256: hash, bytes: content }
    })
    if (parsedFiles.some((file, index) => index > 0 && file.path <= (parsedFiles[index - 1]?.path ?? '')))
      throw badRequest('files', 'invalid')
    const updatedAt = entry.updatedAt
    if (typeof updatedAt !== 'number' || !Number.isSafeInteger(updatedAt)) throw badRequest('updatedAt', 'invalid')
    const skill: SkillRecord = {
      id: text(entry.id, 'id'),
      name: text(entry.name, 'name'),
      description: text(entry.description, 'description'),
      version: entry.version === null ? null : text(entry.version, 'version'),
      body: text(entry.body, 'body'),
      license: entry.license === null ? null : text(entry.license, 'license'),
      extra,
      files: parsedFiles,
      origin,
      revision: revision(entry.revision),
      digest: text(entry.digest, 'digest'),
      updatedAt,
    }
    if (ids.has(skill.id) || names.has(skill.name) || !/^[0-9a-f]{64}$/u.test(skill.digest))
      throw badRequest('skills', 'invalid_document')
    ids.add(skill.id)
    names.add(skill.name)
    validateSkillContent(skill)
    return skill
  })
}
