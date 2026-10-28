import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { KitFailure } from '@ouraihub/aether-core'
import type { Actor } from '../context.js'
import type { FileStore, StoredFile } from '../files.js'
import type { DocumentStore, StoredDocument } from '../storage.js'
import { SkillsService } from './service.js'

const actor: Actor = {
  applicationId: 'app',
  tenantId: 'tenant',
  subjectId: 'alice',
  kind: 'user',
  sessionId: null,
  scopes: [],
}
const key = (owner: Actor, id: string) => JSON.stringify([owner.applicationId, owner.tenantId, owner.subjectId, id])
class Documents implements DocumentStore {
  readonly values = new Map<string, StoredDocument>()
  failNext = false
  loseAcknowledgement = false
  async get(owner: Actor, collection: string, id: string) {
    return this.values.get(key(owner, `${collection}/${id}`)) ?? null
  }
  async list(owner: Actor, collection: string) {
    const prefix = JSON.stringify([owner.applicationId, owner.tenantId, owner.subjectId]).slice(0, -1)
    return [...this.values.entries()]
      .filter(([id]) => id.startsWith(prefix) && id.includes(`${collection}/`))
      .map(([, value]) => value)
  }
  async compareAndSet(owner: Actor, collection: string, id: string, expected: number | null, value: unknown) {
    const storageKey = key(owner, `${collection}/${id}`)
    const current = this.values.get(storageKey)
    if (this.failNext || (current?.revision ?? null) !== expected) {
      this.failNext = false
      throw new KitFailure('conflict', 'CAS conflict')
    }
    const next = { key: id, revision: (current?.revision ?? 0) + 1, value: structuredClone(value) }
    this.values.set(storageKey, next)
    if (this.loseAcknowledgement) {
      this.loseAcknowledgement = false
      throw new KitFailure('internal', 'Commit acknowledgement lost')
    }
    return next
  }
  async remove(owner: Actor, collection: string, id: string, expected: number) {
    const storageKey = key(owner, `${collection}/${id}`)
    if (this.values.get(storageKey)?.revision !== expected) throw new KitFailure('conflict', 'CAS conflict')
    this.values.delete(storageKey)
  }
}
class Blobs implements FileStore {
  readonly values = new Map<string, StoredFile>()
  async put(owner: Actor, id: string, file: { readonly mime: string; readonly data: Uint8Array }) {
    const storageKey = key(owner, id)
    if (this.values.has(storageKey)) throw new KitFailure('conflict', 'Immutable')
    const value = { id, mime: file.mime, data: file.data.slice(), size: file.data.length }
    this.values.set(storageKey, value)
    return value
  }
  async get(owner: Actor, id: string) {
    return this.values.get(key(owner, id)) ?? null
  }
  async remove(owner: Actor, id: string) {
    this.values.delete(key(owner, id))
  }
  url(owner: Actor, id: string) {
    return `https://example.test/files/${encodeURIComponent(key(owner, id))}`
  }
}

describe('actor-scoped skill persistence', () => {
  it('retains old immutable blobs and rolls back unreferenced blobs on failed catalog CAS', async () => {
    const docs = new Documents()
    const blobs = new Blobs()
    const service = new SkillsService(docs, blobs)
    const input = { name: 'guide', description: 'test', body: 'original' }
    await service.save(actor, input, 'one', 1)
    const pinned = await service.snapshot(actor, [{ id: 'one', revision: 1 }], 2)
    const before = blobs.values.size
    docs.failNext = true
    await assert.rejects(
      service.save(actor, { ...input, id: 'one', expectedRevision: 1, body: 'not committed' }, '', 3),
      { code: 'conflict' },
    )
    assert.equal(blobs.values.size, before)
    assert.equal((await service.get(actor, 'one', 4)).body, 'original')
    await service.save(actor, { ...input, id: 'one', expectedRevision: 1, body: 'committed' }, '', 5)
    assert.equal(blobs.values.size, before + 1)
    assert.equal(pinned[0]?.body, 'original')
    await assert.rejects(service.snapshot(actor, [{ id: 'one', revision: 1 }], 6), {
      details: { reason: 'skill_changed', entity: 'one' },
    })
    assert.deepEqual(await service.list({ ...actor, subjectId: 'bob' }, 6), [])
    assert.deepEqual(await service.list({ ...actor, applicationId: 'other' }, 6), [])
    assert.deepEqual(await service.list({ ...actor, tenantId: 'other' }, 6), [])
  })
  it('preserves newly referenced blobs when CAS commits but its acknowledgement fails', async () => {
    const docs = new Documents()
    const blobs = new Blobs()
    const service = new SkillsService(docs, blobs)
    await service.save(actor, { name: 'guide', description: 'test', body: 'original' }, 'one', 1)
    docs.loseAcknowledgement = true
    await assert.rejects(
      service.save(
        actor,
        { id: 'one', expectedRevision: 1, name: 'guide', description: 'test', body: 'committed' },
        '',
        2,
      ),
      { code: 'internal' },
    )
    assert.equal(blobs.values.size, 2)
    assert.equal((await service.get(actor, 'one', 3)).body, 'committed')
  })
})
