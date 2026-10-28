import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { ModelSave, ProviderSave } from '@ouraihub/aether-contracts'
import { KitFailure } from '@ouraihub/aether-core'
import type { Actor, WorkerContext } from '../context.js'
import { createCredentialPort } from '../credentials.js'
import type { DocumentStore, StoredDocument } from '../storage.js'
import { ProviderService } from './index.js'

const actor: Actor = {
  subjectId: 'subject',
  kind: 'user',
  applicationId: 'app',
  tenantId: 'tenant',
  sessionId: 'session',
  scopes: ['*'],
}
class TestDocuments implements DocumentStore {
  readonly documents = new Map<string, StoredDocument>()
  fail = false
  private id(owner: Actor, collection: string, key: string): string {
    return JSON.stringify([owner.applicationId, owner.tenantId, owner.subjectId, collection, key])
  }
  async get(owner: Actor, collection: string, key: string): Promise<StoredDocument | null> {
    return structuredClone(this.documents.get(this.id(owner, collection, key)) ?? null)
  }
  async list(owner: Actor, collection: string): Promise<readonly StoredDocument[]> {
    return [...this.documents.entries()]
      .filter(([key]) =>
        key.startsWith(JSON.stringify([owner.applicationId, owner.tenantId, owner.subjectId, collection]).slice(0, -1)),
      )
      .map(([, value]) => structuredClone(value))
  }
  async compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expectedRevision: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    const id = this.id(owner, collection, key)
    const old = this.documents.get(id)
    if (this.fail || (old?.revision ?? null) !== expectedRevision) throw new KitFailure('conflict', 'CAS conflict')
    const saved = { key, revision: (old?.revision ?? 0) + 1, value: structuredClone(value) }
    this.documents.set(id, saved)
    return structuredClone(saved)
  }
  async remove(owner: Actor, collection: string, key: string, expectedRevision: number): Promise<void> {
    const id = this.id(owner, collection, key)
    if (this.documents.get(id)?.revision !== expectedRevision) throw new KitFailure('conflict', 'CAS conflict')
    this.documents.delete(id)
  }
}
const model: ModelSave = {
  id: 'gpt-5',
  modelType: 'chat',
  typeSource: 'inferred',
  contextWindow: 128000,
  maxOutput: 4096,
  canVision: true,
  supportsTools: true,
  nativeWebSearch: false,
  name: null,
  groupName: null,
  pricing: null,
}
async function setup() {
  const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'])
  const documents = new TestDocuments()
  const credentials = createCredentialPort({ currentKeyId: 'test', keys: new Map([['test', key]]) })
  let reveals = 0
  const service = new ProviderService({
    documents,
    credentials,
    outbound: {
      async fetch() {
        throw new Error('No outbound expected')
      },
    },
    async authorizeReveal() {
      reveals++
      throw new KitFailure('forbidden', 'Fresh authentication required')
    },
  })
  const input: ProviderSave = {
    name: 'Provider',
    endpoints: [{ slug: 'primary', baseUrl: 'https://models.example', protocol: 'openai-chat', models: [model] }],
  }
  const providers = await service.save(actor, input)
  const provider = providers[0]
  const endpoint = provider?.endpoints[0]
  assert.ok(provider)
  assert.ok(endpoint)
  return { service, documents, provider, endpoint, reveals: () => reveals }
}
test('provider CRUD encrypts credentials and reserves one round-robin start per snapshot', async () => {
  const { service, documents, endpoint } = await setup()
  await service.saveKey(actor, { endpointId: endpoint.id, key: 'alpha-secret-value' })
  await service.saveKey(actor, { endpointId: endpoint.id, key: 'bravo-secret-value' })
  assert.equal(JSON.stringify([...documents.documents.values()]).includes('alpha-secret-value'), false)
  const one = await service.resolve(actor, { endpointId: endpoint.id, modelId: model.id })
  const two = await service.resolve(actor, { endpointId: endpoint.id, modelId: model.id })
  assert.deepEqual(one.keys, ['alpha-secret-value', 'bravo-secret-value'])
  assert.deepEqual(two.keys, ['bravo-secret-value', 'alpha-secret-value'])
  assert.equal(one.reasoning?.dialect, 'openai')
  const listed = await service.list(actor)
  const savedKey = listed[0]?.endpoints[0]?.keys[0]
  assert.ok(savedKey)
  assert.equal(savedKey.masked, 'alph…alue')
  assert.equal(savedKey.readable, true)
  await assert.rejects(service.saveKey(actor, { endpointId: endpoint.id, key: 'alpha-secret-value' }), /duplicate/)
  const other: Actor = { ...actor, subjectId: 'other' }
  assert.deepEqual(await service.list(other), [])
  await assert.rejects(service.revealKey(other, savedKey.id), /Unknown key/)
})
test('failed CAS preserves provider configuration and credential', async () => {
  const { service, documents, provider, endpoint } = await setup()
  const list = await service.saveKey(actor, { endpointId: endpoint.id, key: 'original-secret' })
  const keyId = list[0]?.endpoints[0]?.keys[0]?.id
  assert.ok(keyId)
  documents.fail = true
  await assert.rejects(
    service.saveKey(actor, { endpointId: endpoint.id, id: keyId, key: 'replacement-secret' }),
    /CAS conflict/,
  )
  documents.fail = false
  assert.equal(await service.revealKey(actor, keyId), 'original-secret')
  assert.equal((await service.list(actor))[0]?.name, provider.name)
})
test('whole-provider replacement preserves tri-state configuration, manual reasoning disable and key ownership', async () => {
  const { service, provider, endpoint } = await setup()
  await service.saveKey(actor, { endpointId: endpoint.id, key: 'provider-secret' })
  await service.save(actor, {
    id: provider.id,
    name: provider.name,
    endpoints: [
      {
        id: endpoint.id,
        slug: endpoint.slug,
        baseUrl: endpoint.baseUrl,
        protocol: endpoint.protocol,
        imageBaseUrl: 'https://images.example',
        headers: { 'x-team': 'a' },
        models: [{ ...model, reasoning: null }],
      },
    ],
  })
  const keep = await service.save(actor, {
    id: provider.id,
    name: provider.name,
    endpoints: [
      { id: endpoint.id, slug: endpoint.slug, baseUrl: endpoint.baseUrl, protocol: endpoint.protocol, models: [model] },
    ],
  })
  assert.equal(keep[0]?.endpoints[0]?.imageBaseUrl, 'https://images.example')
  assert.deepEqual(keep[0]?.endpoints[0]?.headers, { 'x-team': 'a' })
  assert.equal((await service.resolve(actor, { endpointId: endpoint.id, modelId: model.id })).reasoning, null)
  await service.save(actor, { id: provider.id, name: provider.name, endpoints: [] })
  await assert.rejects(service.resolve(actor, { endpointId: endpoint.id, modelId: model.id }), /Unknown model/)
})
test('preset adoption preserves user data and reveal HTTP requires fresh authentication', async () => {
  const { service, provider, endpoint, reveals } = await setup()
  const adopted = await service.save(actor, { presetId: 'public-preset', name: provider.name, endpoints: [] })
  assert.equal(adopted.length, 1)
  assert.equal(adopted[0]?.presetId, 'public-preset')
  assert.equal(adopted[0]?.endpoints.length, 1)
  await assert.rejects(service.remove(actor, provider.id), /preset/)
  const saved = await service.saveKey(actor, { endpointId: endpoint.id, key: 'secret-value' })
  const keyId = saved[0]?.endpoints[0]?.keys[0]?.id
  assert.ok(keyId)
  const context: WorkerContext = { actor, signal: new AbortController().signal, requestId: 'request', now: Date.now() }
  await assert.rejects(
    service.handle(
      new Request(`https://worker.example/api/v1/providers/keys/${keyId}/reveal`, { method: 'POST', body: '{}' }),
      context,
    ),
    /Fresh authentication/,
  )
  assert.equal(reveals(), 1)
})
test('one unreadable enabled provider key blocks snapshot and partition crossing cannot open it', async () => {
  const { service, documents, endpoint } = await setup()
  const saved = await service.saveKey(actor, { endpointId: endpoint.id, key: 'original-secret' })
  const keyId = saved[0]?.endpoints[0]?.keys[0]?.id
  assert.ok(keyId)
  const stored = await documents.get(actor, 'providers', 'catalog')
  assert.ok(stored)
  const target: Actor = { ...actor, tenantId: 'different-tenant' }
  await documents.compareAndSet(target, 'providers', 'catalog', null, stored.value)
  const listed = await service.list(target)
  assert.equal(listed[0]?.endpoints[0]?.keys[0]?.readable, false)
  await assert.rejects(
    service.resolve(target, { endpointId: endpoint.id, modelId: model.id }),
    /Credential cannot be read/,
  )
  await assert.rejects(service.revealKey(target, keyId), /Credential cannot be read/)
  assert.equal(await service.revealKey(actor, keyId), 'original-secret')
})
test('required model capability is checked before the runtime consumes a snapshot', async () => {
  const { service, endpoint } = await setup()
  await assert.rejects(
    service.resolve(actor, { endpointId: endpoint.id, modelId: model.id }, 'image'),
    /unsupported_type/,
  )
})
