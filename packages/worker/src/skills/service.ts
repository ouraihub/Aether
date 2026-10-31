import type { ManualSave, SkillRef } from '@ouraihub/aether-contracts'
import { badRequest, KitFailure } from '@ouraihub/aether-core'
import { readSkillSource } from '@ouraihub/aether-core/skills/package'
import * as skills from '@ouraihub/aether-core/skills/service'
import {
  exact,
  object,
  parseManualSave,
  revision,
  strings,
  text,
} from '@ouraihub/aether-core/skills/validation'
import type { Actor, WorkerContext } from '../context.js'
import type { DocumentStore } from '../storage.js'
import type { FileStore } from '../files.js'
import {
  loadSkills,
  parseRefs,
  serializeSkill,
  type LoadedSkills,
  type SkillBlobRef,
} from './persistence.js'

/** All catalog mutations commit in one actor-scoped CAS; conflicts never overwrite newer content. */
export class SkillsService {
  private readonly store: DocumentStore
  private readonly blobs: FileStore
  private readonly builtins: readonly skills.BuiltinSkill[]
  constructor(
    store: DocumentStore,
    blobs: FileStore,
    builtins: readonly skills.BuiltinSkill[] = [],
  ) {
    this.store = store
    this.blobs = blobs
    this.builtins = builtins
  }

  private async commit(
    actor: Actor,
    previous: LoadedSkills,
    next: skills.SkillCatalog,
    expected: number | null,
  ): Promise<{ refs: readonly SkillBlobRef[]; revision: number }> {
    const created: string[] = []
    const refs: SkillBlobRef[] = []
    try {
      for (const skill of next) {
        const oldRef = previous.refs.find((ref) => ref.id === skill.id)
        if (
          oldRef &&
          previous.catalog.find((entry) => entry.id === skill.id) === skill
        ) {
          refs.push(oldRef)
          continue
        }
        const blobId = crypto.randomUUID()
        await this.blobs.put(actor, blobId, {
          mime: 'application/json',
          data: serializeSkill(skill),
        })
        created.push(blobId)
        refs.push({ ...skills.metadata(skill), blobId, digest: skill.digest })
      }
      const saved = await this.store.compareAndSet(
        actor,
        'skills',
        'catalog',
        expected,
        refs,
      )
      return { refs, revision: saved.revision }
    } catch (error) {
      // Only a definite rejection proves these blobs are not live catalog references.
      if (error instanceof KitFailure && error.code === 'conflict') {
        await Promise.all(created.map((id) => this.blobs.remove(actor, id)))
      }
      throw error
    }
  }
  private async load(
    actor: Actor,
    now: number,
  ): Promise<LoadedSkills & { revision: number | null }> {
    const document = await this.store.get(actor, 'skills', 'catalog')
    const current =
      document === null
        ? { catalog: [], refs: [] }
        : await loadSkills(this.blobs, actor, document.value)
    const synced = await skills.syncBuiltins(
      current.catalog,
      this.builtins,
      now,
    )
    if (synced === current.catalog)
      return { ...current, revision: document?.revision ?? null }
    const saved = await this.commit(
      actor,
      current,
      synced,
      document?.revision ?? null,
    )
    return { catalog: synced, ...saved }
  }
  private async mutate(
    actor: Actor,
    now: number,
    operation: (
      catalog: skills.SkillCatalog,
    ) => Promise<skills.SkillCatalog> | skills.SkillCatalog,
  ) {
    const current = await this.load(actor, now)
    const next = await operation(current.catalog)
    await this.commit(actor, current, next, current.revision)
    return skills.list(next)
  }
  async list(actor: Actor, now: number) {
    if (this.builtins.length !== 0)
      return skills.list((await this.load(actor, now)).catalog)
    const document = await this.store.get(actor, 'skills', 'catalog')
    const refs = document === null ? [] : parseRefs(document.value)
    if (refs.some((ref) => ref.origin === 'builtin'))
      return skills.list((await this.load(actor, now)).catalog)
    return refs.map((ref) => ({
      id: ref.id,
      name: ref.name,
      description: ref.description,
      version: ref.version,
      origin: ref.origin,
      revision: ref.revision,
      bodyChars: ref.bodyChars,
      updatedAt: ref.updatedAt,
      hasScripts: ref.hasScripts,
      fileCount: ref.fileCount,
    }))
  }
  async get(actor: Actor, id: string, now: number) {
    return skills.get((await this.load(actor, now)).catalog, id)
  }
  async files(actor: Actor, id: string, expected: number, now: number) {
    return skills.files((await this.load(actor, now)).catalog, id, expected)
  }
  async file(
    actor: Actor,
    id: string,
    expected: number,
    path: string,
    now: number,
  ) {
    return skills.file(
      (await this.load(actor, now)).catalog,
      id,
      expected,
      path,
    )
  }
  async snapshot(actor: Actor, refs: readonly SkillRef[], now: number) {
    return skills.snapshot((await this.load(actor, now)).catalog, refs)
  }
  async preview(actor: Actor, source: unknown, now: number) {
    const content = await readSkillSource(source)
    return skills.preview((await this.load(actor, now)).catalog, content)
  }
  async install(
    actor: Actor,
    source: unknown,
    confirmedDigest: string,
    replace: boolean,
    newId: string,
    now: number,
  ) {
    const content = await readSkillSource(source)
    return this.mutate(actor, now, (catalog) =>
      skills.install(catalog, content, confirmedDigest, replace, newId, now),
    )
  }
  async save(actor: Actor, input: ManualSave, newId: string, now: number) {
    const checked = parseManualSave(input)
    return this.mutate(actor, now, (catalog) =>
      skills.save(catalog, checked, newId, now),
    )
  }
  async duplicate(
    actor: Actor,
    id: string,
    newName: string,
    newId: string,
    now: number,
  ) {
    return this.mutate(actor, now, (catalog) =>
      skills.duplicate(catalog, id, newName, newId, now),
    )
  }
  async reorder(actor: Actor, ids: readonly string[], now: number) {
    return this.mutate(actor, now, (catalog) => skills.reorder(catalog, ids))
  }
  async remove(actor: Actor, id: string, now: number) {
    return this.mutate(actor, now, (catalog) => skills.remove(catalog, id))
  }

  async handle(
    request: Request,
    context: WorkerContext,
  ): Promise<Response | null> {
    const url = new URL(request.url)
    const base = '/api/v1/skills'
    if (url.pathname !== base && !url.pathname.startsWith(`${base}/`))
      return null
    const { actor, now } = context
    if (context.signal.aborted) throw new KitFailure('cancelled', 'Cancelled')
    const path = url.pathname.slice(base.length)
    if (request.method === 'GET') {
      if (path === '') return Response.json(await this.list(actor, now))
      const parts = path.slice(1).split('/')
      const id = decodeURIComponent(parts[0] ?? '')
      if (parts.length === 1)
        return Response.json(await this.get(actor, id, now))
      const expected = revision(Number(url.searchParams.get('revision')))
      if (parts.length === 2 && parts[1] === 'files')
        return Response.json(await this.files(actor, id, expected, now))
      if (parts.length === 2 && parts[1] === 'file') {
        const bytes = await this.file(
          actor,
          id,
          expected,
          text(url.searchParams.get('path'), 'path'),
          now,
        )
        return new Response(bytes, {
          headers: {
            'Content-Type': 'application/octet-stream',
            'X-Content-Type-Options': 'nosniff',
          },
        })
      }
    }
    if (request.method === 'DELETE' && /^\/[^/]+$/u.test(path))
      return Response.json(
        await this.remove(actor, decodeURIComponent(path.slice(1)), now),
      )
    if (request.method === 'POST') {
      let raw: unknown
      try {
        raw = await request.json()
      } catch {
        throw badRequest('body', 'invalid_json')
      }
      if (path === '/save')
        return Response.json(
          await this.save(
            actor,
            parseManualSave(raw),
            crypto.randomUUID(),
            now,
          ),
        )
      const body = object(raw, 'body')
      if (path === '/preview') {
        exact(body, ['source'], 'body')
        return Response.json(await this.preview(actor, body.source, now))
      }
      if (path === '/install') {
        exact(body, ['source', 'confirmedDigest', 'replace'], 'body')
        if (typeof body.replace !== 'boolean')
          throw badRequest('replace', 'invalid')
        return Response.json(
          await this.install(
            actor,
            body.source,
            text(body.confirmedDigest, 'confirmedDigest'),
            body.replace,
            crypto.randomUUID(),
            now,
          ),
        )
      }
      if (path === '/reorder') {
        exact(body, ['ids'], 'body')
        return Response.json(
          await this.reorder(actor, strings(body.ids, 'ids'), now),
        )
      }
      if (path === '/snapshot') {
        exact(body, ['refs'], 'body')
        const refs: unknown = body.refs
        if (!Array.isArray(refs)) throw badRequest('refs', 'invalid')
        const checked = refs.map((value: unknown) => {
          const ref = object(value, 'ref')
          exact(ref, ['id', 'revision'], 'ref')
          return { id: text(ref.id, 'id'), revision: revision(ref.revision) }
        })
        return Response.json(await this.snapshot(actor, checked, now))
      }
      const duplicate = /^\/([^/]+)\/duplicate$/u.exec(path)
      if (duplicate?.[1] !== undefined) {
        exact(body, ['newName'], 'body')
        return Response.json(
          await this.duplicate(
            actor,
            decodeURIComponent(duplicate[1]),
            text(body.newName, 'newName'),
            crypto.randomUUID(),
            now,
          ),
        )
      }
    }
    throw badRequest('route', 'unsupported')
  }
}
