import assert from 'node:assert/strict'
import { test } from 'node:test'
import { KitFailure } from '@ouraihub/aether-core/error'
import { SearchService } from './index.js'
import { ownerKey, type Actor, type WorkerContext } from '../context.js'
import { createCredentialPort, type CredentialPort } from '../credentials.js'
import type { DocumentStore, StoredDocument } from '../storage.js'

const actor: Actor = {
  applicationId: 'app',
  tenantId: null,
  subjectId: 'owner',
  sessionId: 'session',
  kind: 'user',
  scopes: [],
}
class Documents implements DocumentStore {
  readonly records = new Map<string, StoredDocument>()
  rejectNext = false
  private address(owner: Actor, collection: string, key: string): string {
    return JSON.stringify([ownerKey(owner), collection, key])
  }
  async get(owner: Actor, collection: string, key: string): Promise<StoredDocument | null> {
    return structuredClone(this.records.get(this.address(owner, collection, key)) ?? null)
  }
  async list(owner: Actor, collection: string): Promise<readonly StoredDocument[]> {
    const prefix = JSON.stringify([ownerKey(owner), collection]).slice(0, -1) + ','
    return [...this.records]
      .filter(([address]) => address.startsWith(prefix))
      .map(([, value]) => structuredClone(value))
  }
  async compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expectedRevision: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    const address = this.address(owner, collection, key)
    const old = this.records.get(address)
    if (this.rejectNext) {
      this.rejectNext = false
      throw new KitFailure('conflict', 'Injected CAS conflict')
    }
    if ((old?.revision ?? null) !== expectedRevision) throw new KitFailure('conflict', 'CAS conflict')
    const document = { key, revision: (old?.revision ?? 0) + 1, value: structuredClone(value) }
    this.records.set(address, document)
    return structuredClone(document)
  }
  async remove(owner: Actor, collection: string, key: string, expectedRevision: number): Promise<void> {
    const address = this.address(owner, collection, key)
    if (this.records.get(address)?.revision !== expectedRevision) throw new KitFailure('conflict', 'CAS conflict')
    this.records.delete(address)
  }
}
async function fixture() {
  const encryptionKey = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'])
  const credentials = createCredentialPort({ currentKeyId: 'current', keys: new Map([['current', encryptionKey]]) })
  const documents = new Documents()
  let verificationCalls = 0
  const options = {
    documents,
    credentials,
    outbound: {
      async fetch() {
        return Response.json({ results: [{ title: 'Source', url: 'https://source.test/', content: 'content' }] })
      },
    },
    checkTarget: async () => {},
    verifyCurrentPassword: async (_actor: Actor, password: string) => {
      verificationCalls++
      if (password !== 'correct') throw new KitFailure('unauthorized', 'Fresh authentication required')
    },
  }
  return {
    documents,
    credentials,
    options,
    service: new SearchService(options),
    verifications: () => verificationCalls,
  }
}

test('encrypted configuration survives new service, keeps partitions isolated, and requires fresh authentication', async () => {
  const { service, options, documents, verifications } = await fixture()
  await service.saveKey(actor, 'search', 'querit', 'search-secret')
  await service.saveKey(actor, 'fetch', 'querit', 'fetch-secret')
  const settings = await service.settings(actor, 'search')
  const key = settings.providers.find((provider) => provider.id === 'querit')?.keys[0]
  assert.ok(key)
  assert.equal(JSON.stringify([...documents.records]).includes('search-secret'), false)
  assert.equal(key.masked.includes('search-secret'), false)
  const restarted = new SearchService(options)
  assert.deepEqual((await restarted.resolve(actor, 'search', 'querit')).apiKeys, ['search-secret'])
  assert.deepEqual((await restarted.resolve(actor, 'fetch', 'querit')).apiKeys, ['fetch-secret'])
  await assert.rejects(restarted.reveal(actor, 'search', 'querit', key.id, 'wrong', new AbortController().signal), {
    code: 'unauthorized',
  })
  assert.equal(
    await restarted.reveal(actor, 'search', 'querit', key.id, 'correct', new AbortController().signal),
    'search-secret',
  )
  assert.equal(verifications(), 2)
  await assert.rejects(restarted.removeKey(actor, 'fetch', 'querit', key.id), { code: 'not_found' })
  assert.equal(
    (await restarted.settings({ ...actor, subjectId: 'other' }, 'search')).providers.find(
      (provider) => provider.id === 'querit',
    )?.keys.length,
    0,
  )
})

test('resolve rotates once in each capability/provider and existing snapshots remain unchanged', async () => {
  const { service } = await fixture()
  for (const capability of ['search', 'fetch'] as const)
    for (const key of ['first', 'second', 'third']) await service.saveKey(actor, capability, 'querit', capability + key)
  const first = await service.resolve(actor, 'search', 'querit')
  assert.deepEqual(first.apiKeys, ['searchfirst', 'searchsecond', 'searchthird'])
  assert.deepEqual((await service.resolve(actor, 'search', 'querit')).apiKeys, [
    'searchsecond',
    'searchthird',
    'searchfirst',
  ])
  assert.deepEqual((await service.resolve(actor, 'fetch', 'querit')).apiKeys, [
    'fetchfirst',
    'fetchsecond',
    'fetchthird',
  ])
  const id = (await service.settings(actor, 'search')).providers.find((provider) => provider.id === 'querit')?.keys[0]
    ?.id
  assert.ok(id)
  await service.saveKey(actor, 'search', 'querit', 'updated', id)
  assert.deepEqual(first.apiKeys, ['searchfirst', 'searchsecond', 'searchthird'])
})

test('unreadable keys stay visible, partial reads skip bad keys, all unreadable requires configuration', async () => {
  const { service, options, credentials } = await fixture()
  await service.saveKey(actor, 'search', 'querit', 'first')
  await service.saveKey(actor, 'search', 'querit', 'second')
  const bad = (await service.settings(actor, 'search')).providers.find((provider) => provider.id === 'querit')?.keys[0]
    ?.id
  const partial: CredentialPort = {
    ...credentials,
    async open(scope, sealed) {
      if (scope.recordId.includes(bad ?? '')) throw new Error('unreadable')
      return credentials.open(scope, sealed)
    },
  }
  const available = new SearchService({ ...options, credentials: partial })
  assert.deepEqual((await available.resolve(actor, 'search', 'querit')).apiKeys, ['second'])
  const provider = (await available.settings(actor, 'search')).providers.find((item) => item.id === 'querit')
  assert.equal(provider?.keys.length, 2)
  assert.equal(provider?.ready, true)
  const unavailable = new SearchService({
    ...options,
    credentials: {
      ...credentials,
      async open() {
        throw new Error('unreadable')
      },
    },
  })
  await assert.rejects(unavailable.resolve(actor, 'search', 'querit'), { code: 'needs_config' })
  assert.equal(
    (await unavailable.settings(actor, 'search')).providers.find((item) => item.id === 'querit')?.ready,
    false,
  )
})

test('CAS failure retains prior encrypted key and sixteen-key boundary is atomic', async () => {
  const { service, documents } = await fixture()
  await service.saveKey(actor, 'search', 'querit', 'old-value')
  const id = (await service.settings(actor, 'search')).providers.find((provider) => provider.id === 'querit')?.keys[0]
    ?.id
  assert.ok(id)
  documents.rejectNext = true
  await assert.rejects(service.saveKey(actor, 'search', 'querit', 'new-value', id), { code: 'conflict' })
  assert.deepEqual((await service.resolve(actor, 'search', 'querit')).apiKeys, ['old-value'])
  for (let index = 1; index < 16; index++) await service.saveKey(actor, 'search', 'querit', 'key-' + index)
  await assert.rejects(service.saveKey(actor, 'search', 'querit', 'seventeenth'), { code: 'bad_request' })
  assert.equal(
    (await service.settings(actor, 'search')).providers.find((provider) => provider.id === 'querit')?.keys.length,
    16,
  )
})

test('management HTTP returns raw DTOs and rejects unknown request fields', async () => {
  const { service } = await fixture()
  const context: WorkerContext = { actor, signal: new AbortController().signal, requestId: 'request', now: 0 }
  const response = await service.handle(
    new Request('https://host.test/api/v1/search/settings?capability=search'),
    context,
  )
  assert.ok(response)
  const value: unknown = await response.json()
  assert.ok(typeof value === 'object' && value !== null && 'capability' in value && value.capability === 'search')
  await assert.rejects(
    service.handle(
      new Request('https://host.test/api/v1/search/default', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ capability: 'search', provider: 'exa-mcp', injected: true }),
      }),
      context,
    ),
    { code: 'bad_request' },
  )
  assert.equal(await service.handle(new Request('https://host.test/api/v1/unrelated'), context), null)
})

test('explicit credential rotation rewraps sealed values without changing plaintext or partition', async () => {
  const { service, options, credentials } = await fixture()
  await service.saveKey(actor, 'search', 'querit', 'rotate-me')
  const nextKey = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'])
  const next = createCredentialPort({ currentKeyId: 'next', keys: new Map([['next', nextKey]]) })
  const rotating: CredentialPort = {
    seal: next.seal,
    open: credentials.open,
    async rotate(scope, sealed) {
      const plaintext = await credentials.open(scope, sealed)
      try {
        return await next.seal(scope, plaintext)
      } finally {
        plaintext.fill(0)
      }
    },
  }
  await new SearchService({ ...options, credentials: rotating }).rotateCredentials(actor, 'search', 'querit')
  const reopened = new SearchService({ ...options, credentials: next })
  assert.deepEqual((await reopened.resolve(actor, 'search', 'querit')).apiKeys, ['rotate-me'])
  assert.equal((await reopened.settings(actor, 'fetch')).providers.find((item) => item.id === 'querit')?.keys.length, 0)
})

test('configuration omission preserves values, empty restores defaults, check uses the independent execution path', async () => {
  const { service, options } = await fixture()
  const signal = new AbortController().signal
  await service.save(
    actor,
    { capability: 'search', provider: 'searxng', baseUrl: 'https://self.test', basicAuthUsername: 'user' },
    signal,
  )
  await service.save(actor, { capability: 'search', provider: 'searxng' }, signal)
  let provider = (await service.settings(actor, 'search')).providers.find((item) => item.id === 'searxng')
  assert.equal(provider?.baseUrl, 'https://self.test')
  assert.equal(provider?.basicAuthUsername, 'user')
  await service.save(actor, { capability: 'search', provider: 'searxng', baseUrl: '', basicAuthUsername: '' }, signal)
  provider = (await service.settings(actor, 'search')).providers.find((item) => item.id === 'searxng')
  assert.equal(provider?.baseUrl, undefined)
  assert.equal(provider?.basicAuthUsername, undefined)
  await service.saveKey(actor, 'search', 'tavily', 'key')
  const requests: RequestInit[] = []
  const checking = new SearchService({
    ...options,
    outbound: {
      async fetch(_url, init) {
        requests.push(init)
        return Response.json({ results: [{ url: 'https://source.test/', content: 'body' }] })
      },
    },
  })
  assert.deepEqual(await checking.check(actor, 'search', 'tavily'), { count: 1 })
  assert.equal(requests[0]?.body, JSON.stringify({ query: 'web search', max_results: 5 }))
  await assert.rejects(
    new SearchService({
      ...options,
      outbound: {
        async fetch() {
          return Response.json({ results: [] })
        },
      },
    }).check(actor, 'search', 'tavily'),
    { code: 'upstream' },
  )
})
