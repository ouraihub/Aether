import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { DatabaseSync } from 'node:sqlite'
import test from 'node:test'
import type { D1Database, D1Statement } from './cloudflare.ts'
import type { Actor } from './context.ts'
import { D1DocumentStore, validateD1Schema } from './storage.ts'

const actor: Actor = {
  applicationId: 'app',
  tenantId: null,
  subjectId: 'user',
  kind: 'user',
  sessionId: null,
  scopes: [],
}
function database(): {
  readonly sqlite: DatabaseSync
  readonly binding: D1Database
} {
  const sqlite = new DatabaseSync(':memory:')
  sqlite.exec(
    readFileSync(new URL('../migrations/0001.sql', import.meta.url), 'utf8'),
  )
  class Statement implements D1Statement {
    readonly #sql: string
    #values: (string | number | null)[] = []
    constructor(sql: string) {
      this.#sql = sql
    }
    bind(...values: (string | number | null)[]): D1Statement {
      this.#values = values
      return this
    }
    async first(): Promise<unknown> {
      return sqlite.prepare(this.#sql).get(...this.#values) ?? null
    }
    async all(): Promise<{ results: readonly unknown[]; success: boolean }> {
      return {
        results: sqlite.prepare(this.#sql).all(...this.#values),
        success: true,
      }
    }
  }
  return {
    sqlite,
    binding: {
      prepare(sql: string) {
        return new Statement(sql)
      },
    },
  }
}
test('real SQL isolates every owner dimension and enforces atomic CAS/tombstones', async () => {
  const { sqlite, binding } = database()
  try {
    await validateD1Schema(binding)
    const store = new D1DocumentStore(binding)
    const created = await store.compareAndSet(actor, 'settings', 'key', null, {
      encrypted: 'value',
    })
    assert.equal(created.revision, 1)
    for (const other of [
      { ...actor, applicationId: 'other' },
      { ...actor, tenantId: '' },
      { ...actor, tenantId: 'tenant' },
      { ...actor, subjectId: 'other' },
    ]) {
      if (other.tenantId === '') {
        await assert.rejects(store.get(other, 'settings', 'key'))
        continue
      }
      assert.equal(await store.get(other, 'settings', 'key'), null)
      assert.deepEqual(await store.list(other, 'settings'), [])
    }
    const race = await Promise.allSettled([
      store.compareAndSet(actor, 'settings', 'key', 1, { n: 1 }),
      store.compareAndSet(actor, 'settings', 'key', 1, { n: 2 }),
    ])
    assert.equal(
      race.filter((result) => result.status === 'fulfilled').length,
      1,
    )
    assert.equal((await store.get(actor, 'settings', 'key'))?.revision, 2)
    await assert.rejects(store.remove(actor, 'settings', 'key', 1), {
      code: 'conflict',
    })
    await store.remove(actor, 'settings', 'key', 2)
    assert.equal(await store.get(actor, 'settings', 'key'), null)
    await assert.rejects(
      store.compareAndSet(actor, 'settings', 'key', null, {}),
      { code: 'conflict' },
    )
    await assert.rejects(store.compareAndSet(actor, 'settings', 'key', 2, {}), {
      code: 'conflict',
    })
    assert.deepEqual(await store.list(actor, 'settings'), [])
    assert.equal(
      sqlite.prepare('SELECT value_json FROM aether_documents').get()
        ?.value_json,
      null,
    )
  } finally {
    sqlite.close()
  }
})
test('schema assertions never run migrations and reject future schema', async () => {
  const { sqlite, binding } = database()
  try {
    sqlite.exec('UPDATE aether_schema SET version = 2')
    await assert.rejects(validateD1Schema(binding), { code: 'incompatible_db' })
  } finally {
    sqlite.close()
  }
})
test('documents reject non-JSON and oversized values', async () => {
  const { sqlite, binding } = database()
  try {
    const store = new D1DocumentStore(binding)
    await assert.rejects(
      store.compareAndSet(actor, 'a', 'b', null, undefined),
      { code: 'bad_request' },
    )
    await assert.rejects(
      store.compareAndSet(actor, 'a', 'b', null, 'x'.repeat(256 * 1024)),
      { code: 'needs_config' },
    )
  } finally {
    sqlite.close()
  }
})

test('large documents use immutable owner-scoped R2 pointers and conflict cleanup', async () => {
  const { sqlite, binding } = database()
  const blobs = new Map<string, Uint8Array>()
  const bucket = {
    async put(key: string, bytes: ArrayBuffer | Uint8Array) {
      blobs.set(key, new Uint8Array(bytes))
      return { key }
    },
    async get(key: string) {
      const bytes = blobs.get(key)
      return bytes
        ? {
            size: bytes.byteLength,
            async arrayBuffer() {
              return new Uint8Array(bytes).buffer
            },
          }
        : null
    },
    async delete(key: string) {
      blobs.delete(key)
    },
  }
  try {
    const store = new D1DocumentStore(binding, { blobs: bucket })
    const value = { image: 'x'.repeat(2_100_000) }
    await store.compareAndSet(actor, 'history', 'large', null, value)
    assert.equal(blobs.size, 1)
    const row = sqlite
      .prepare('SELECT value_json, blob_key FROM aether_documents')
      .get()
    assert.equal(row?.value_json, null)
    assert.equal(typeof row?.blob_key, 'string')
    assert.deepEqual((await store.get(actor, 'history', 'large'))?.value, value)
    await assert.rejects(
      store.compareAndSet(actor, 'history', 'large', null, value),
      { code: 'conflict' },
    )
    assert.equal(blobs.size, 1)
    await store.remove(actor, 'history', 'large', 1)
    assert.equal(
      blobs.size,
      1,
      'old blobs are retained for explicit GC, not deleted under readers',
    )
  } finally {
    sqlite.close()
  }
})
test('bounded lists paginate without dropping records after the first page', async () => {
  const { sqlite, binding } = database()
  try {
    const store = new D1DocumentStore(binding)
    for (let i = 0; i < 205; i++)
      await store.compareAndSet(
        actor,
        'many',
        String(i).padStart(3, '0'),
        null,
        i,
      )
    const first = await store.list(actor, 'many')
    assert.equal(first.length, 200)
    const last = first.at(-1)
    assert.ok(last)
    const second = await store.list(actor, 'many', { after: last.key })
    assert.equal(second.length, 5)
    assert.equal(
      new Set([...first, ...second].map((item) => item.key)).size,
      205,
    )
  } finally {
    sqlite.close()
  }
})
