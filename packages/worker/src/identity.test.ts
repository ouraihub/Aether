import assert from 'node:assert/strict'
import test from 'node:test'
import { KitFailure } from '@ouraihub/aether-core'
import { ownerKey, type Actor } from './context.ts'
import { createIdentityPort, OpaqueSessionStore } from './identity.ts'
import type { DocumentStore, StoredDocument } from './storage.ts'

const actor: Actor = {
  applicationId: 'app',
  tenantId: 'tenant',
  subjectId: 'alice',
  kind: 'user',
  sessionId: 'login',
  scopes: ['settings:write'],
}
function sessionDatabase(): { readonly store: DocumentStore; readonly rows: Map<string, StoredDocument | null> } {
  const rows = new Map<string, StoredDocument | null>()
  const store: DocumentStore = {
    async get(owner, collection, key) {
      return rows.get(JSON.stringify([ownerKey(owner), collection, key])) ?? null
    },
    async list() {
      return []
    },
    async compareAndSet(owner, collection, key, expected, value) {
      const id = JSON.stringify([ownerKey(owner), collection, key])
      const before = rows.get(id)
      if (expected === null ? rows.has(id) : !before || before.revision !== expected)
        throw new KitFailure('conflict', 'CAS')
      const document = { key, revision: (before?.revision ?? 0) + 1, value }
      rows.set(id, document)
      return document
    },
    async remove(owner, collection, key, expected) {
      const id = JSON.stringify([ownerKey(owner), collection, key])
      if (rows.get(id)?.revision !== expected) throw new KitFailure('conflict', 'CAS')
      rows.set(id, null)
    },
  }
  return { store, rows }
}
test('identity validates deployment audience and invokes fresh host verification for every reveal', async () => {
  let verifies = 0
  let current = actor
  const identity = createIdentityPort({
    applicationId: 'app',
    async resolve() {
      return current
    },
    async verifyFresh() {
      verifies++
      return true
    },
  })
  const request = new Request('https://app.example.com')
  assert.equal((await identity.resolve(request))?.subjectId, 'alice')
  assert.equal(await identity.verifyFresh(request, actor), true)
  assert.equal(await identity.verifyFresh(request, actor), true)
  assert.equal(verifies, 2)
  current = { ...actor, sessionId: 'other' }
  assert.equal(await identity.verifyFresh(request, actor), false)
  current = { ...actor, applicationId: 'other' }
  assert.equal(await identity.resolve(request), null)
})
test('opaque sessions persist hashes only and expire/revoke/rotate with absolute lifetime preserved', async () => {
  const { store, rows } = sessionDatabase()
  let now = 1000
  const sessions = new OpaqueSessionStore({ store, applicationId: 'app', idleMs: 50, absoluteMs: 200, now: () => now })
  const issued = await sessions.issue(actor)
  assert.equal(JSON.stringify([...rows]).includes(issued.token), false)
  assert.equal((await sessions.resolve(issued.token))?.subjectId, 'alice')
  now = 1020
  const rotated = await sessions.rotate(issued.token)
  assert.ok(rotated)
  assert.equal(rotated.expiresAt, issued.expiresAt)
  assert.equal(await sessions.resolve(issued.token), null)
  assert.equal((await sessions.resolve(rotated.token))?.subjectId, 'alice')
  await sessions.revoke(rotated.token)
  assert.equal(await sessions.resolve(rotated.token), null)
  await sessions.revoke(rotated.token)
  const idle = await sessions.issue(actor)
  now += 50
  assert.equal(await sessions.resolve(idle.token), null)
  const absolute = await sessions.issue(actor)
  now += 200
  assert.equal(await sessions.resolve(absolute.token), null)
  const other = new OpaqueSessionStore({ store, applicationId: 'other', idleMs: 50, absoluteMs: 200, now: () => now })
  assert.equal(await other.resolve(absolute.token), null)
})
