import assert from 'node:assert/strict'
import test from 'node:test'
import type { RunRequest } from '@ouraihub/aether-contracts'
import type { RunInput, RunSummary } from '@ouraihub/aether-core/types'
import { KitFailure } from '@ouraihub/aether-core/error'
import { ConversationService } from './index.ts'
import type { AcceptedHandle, PreparedAction } from './index.ts'
import { ownerKey } from '../context.ts'
import type { Actor } from '../context.ts'
import type { DocumentStore, StoredDocument } from '../storage.ts'
import { COLLECTION, emptyUsage, parseDocument } from './model.ts'

const actor: Actor = {
  applicationId: 'test-app',
  tenantId: null,
  subjectId: 'one',
  kind: 'user',
  sessionId: null,
  scopes: [],
}
class FakeStore implements DocumentStore {
  rows = new Map<string, StoredDocument>()
  beforeCas: (() => Promise<void>) | null = null
  private key(owner: Actor, collection: string, key: string): string {
    return JSON.stringify([ownerKey(owner), collection, key])
  }
  async get(owner: Actor, collection: string, key: string): Promise<StoredDocument | null> {
    return structuredClone(this.rows.get(this.key(owner, collection, key)) ?? null)
  }
  async list(
    owner: Actor,
    collection: string,
    options: { after?: string; limit?: number } = {},
  ): Promise<readonly StoredDocument[]> {
    const prefix = JSON.stringify([ownerKey(owner), collection]).slice(0, -1) + ','
    return structuredClone(
      [...this.rows.entries()]
        .filter(([key]) => key.startsWith(prefix))
        .map(([, row]) => row)
        .filter((row) => options.after === undefined || row.key > options.after)
        .sort((a, b) => a.key.localeCompare(b.key))
        .slice(0, options.limit ?? 200),
    )
  }
  async compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expectedRevision: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    const hook = this.beforeCas
    this.beforeCas = null
    await hook?.()
    const composite = this.key(owner, collection, key),
      prior = this.rows.get(composite)
    if ((prior?.revision ?? null) !== expectedRevision) throw new KitFailure('conflict', 'Revision conflict')
    const row = { key, revision: (prior?.revision ?? 0) + 1, value: structuredClone(value) }
    this.rows.set(composite, row)
    return structuredClone(row)
  }
  async remove(owner: Actor, collection: string, key: string, expectedRevision: number): Promise<void> {
    const composite = this.key(owner, collection, key)
    if (this.rows.get(composite)?.revision !== expectedRevision) throw new KitFailure('conflict', 'Revision conflict')
    this.rows.delete(composite)
  }
}
function request(overrides: Partial<RunRequest> = {}): RunRequest {
  return { purpose: 'chat', skills: [], input: 'question', regenerate: false, language: 'en', ...overrides }
}
function execution(prepared: PreparedAction, runId: string): RunInput {
  return {
    runId,
    purpose: 'chat',
    permissionMode: 'default',
    connection: {
      endpointId: 'endpoint',
      modelId: 'model',
      protocol: 'openai-chat',
      baseUrl: 'https://provider.example',
      keys: ['private'],
      headers: {},
      supportsTools: true,
      canVision: true,
      contextWindow: 32000,
      maxOutput: 4096,
      reasoning: null,
      pricing: null,
    },
    system: [],
    history: [],
    input: prepared.input,
    images: prepared.images,
    tools: [],
    budget: {
      maxOutput: 4096,
      contextWindow: 32000,
      maxRounds: 5,
      toolOutputChars: 8000,
      skillsChars: 48000,
      contextChars: 24000,
    },
    reasoningEffort: 'default',
    nativeWebSearch: false,
  }
}
function summary(text = 'answer'): RunSummary {
  return {
    status: 'completed',
    messages: [
      {
        assistant: {
          text,
          tool_calls: [],
          reasoning: [
            {
              protocol: 'openai-chat',
              endpointId: 'endpoint',
              modelId: 'model',
              value: { privateNativeReasoning: 'must not project' },
            },
          ],
        },
      },
    ],
    usage: { ...emptyUsage(), inputTokens: 15, outputTokens: 7 },
    cost: { amount: 0.1, currency: 'USD' },
    contextUsage: null,
    exhausted: false,
  }
}
function setup(): { service: ConversationService; store: FakeStore; advance: (time: number) => void } {
  const store = new FakeStore()
  let now = 1000,
    id = 0
  return {
    store,
    service: new ConversationService({ store, now: () => now, newId: () => `copy-${++id}` }),
    advance: (time) => {
      now = time
    },
  }
}
async function complete(
  service: ConversationService,
  runId: string,
  req = request(),
  text = 'answer',
): Promise<AcceptedHandle> {
  const prepared = await service.resolve(req, actor),
    reserved = await service.reserve(prepared, execution(prepared, runId), runId, actor)
  await service.finish(reserved.handle, summary(text), actor)
  return reserved.handle
}
function reason(expected: string): (error: unknown) => boolean {
  return (error) => error instanceof KitFailure && error.details?.reason === expected
}

test('pure validation failure leaves no conversation or running placeholder', async () => {
  const { service, store } = setup(),
    prepared = await service.resolve(request(), actor)
  await assert.rejects(
    service.reserve(prepared, execution(prepared, 'run'), 'run', actor, () => {
      throw new KitFailure('budget_exceeded', 'No room')
    }),
  )
  assert.equal(store.rows.size, 0)
  assert.deepEqual(service.plan(prepared, 'run'), {
    runId: 'run',
    conversationId: 'run-conversation',
    turnId: 'run',
    reservationId: 'run',
  })
})

test('actor isolation, final usage/cost projection, and native reasoning privacy', async () => {
  const { service, store } = setup(),
    handle = await complete(service, 'first')
  const wire = await service.get(handle.conversationId, actor)
  assert.equal(wire.turns[0]?.usage?.inputTokens, 15)
  assert.equal(wire.turns[0]?.cost?.amount, 0.1)
  assert.equal(wire.turns[0]?.reasoningText, '')
  assert.ok(!JSON.stringify(wire).includes('privateNativeReasoning'))
  assert.ok(!JSON.stringify([...store.rows.values()]).includes('private"'))
  await assert.rejects(
    service.get(handle.conversationId, { ...actor, subjectId: 'other' }),
    (error: unknown) => error instanceof KitFailure && error.code === 'not_found',
  )
  await assert.rejects(service.get(handle.conversationId, { ...actor, applicationId: 'another-app' }))
  await assert.rejects(service.finish(handle, summary('overwrite'), actor), reason('turn_finished'))
})

test('retry retains candidate identity, original images and creation TTL; translation excludes images', async () => {
  const { service, advance, store } = setup(),
    image = { mime: 'image/png', data: 'AQ==' }
  const first = await complete(service, 'first', request({ images: [image] }), 'visible reply')
  advance(9000)
  const prepared = await service.resolve(
    request({ conversationId: first.conversationId, retryTurnId: first.turnId, input: 'ignored client input' }),
    actor,
  )
  assert.equal(prepared.input, 'question')
  assert.deepEqual(prepared.images, [image])
  assert.deepEqual(prepared.sourceModel, { endpointId: 'endpoint', modelId: 'model' })
  const retry = await service.reserve(prepared, execution(prepared, 'retry'), 'retry', actor)
  await service.finish(retry.handle, summary('new candidate'), actor)
  const stored = await store.get(actor, COLLECTION, first.conversationId)
  assert.ok(stored)
  const document = parseDocument(stored.value, stored.key)
  assert.equal(document.turns[1]?.images[0]?.createdAt, 1000)
  assert.equal(document.turns[1]?.supersedesId, 'first')
  assert.equal(document.turns[1]?.questionId, 'first')
  assert.equal(document.summary.selectedTurnId, 'first')
  const translate = await service.resolve(
    request({ conversationId: first.conversationId, translateTurnId: 'retry', targetLanguage: ' French ' }),
    actor,
  )
  assert.ok(translate.input.includes('into French.'))
  assert.ok(translate.input.endsWith('new candidate'))
  assert.deepEqual(translate.images, [])
  assert.deepEqual(
    translate.historyPath.map((t) => t.id),
    ['retry'],
  )
  await service.purgeImages(1001, actor)
  const expired = await service.resolve(
    request({ conversationId: first.conversationId, retryTurnId: first.turnId }),
    actor,
  )
  assert.deepEqual(expired.images, [])
  assert.equal((await service.get(first.conversationId, actor)).turns[0]?.images[0]?.url, null)
})

test('reservation rejects source edit and image expiry after preparation without partial updates', async () => {
  const { service, store } = setup(),
    first = await complete(service, 'first', request({ images: [{ mime: 'image/png', data: 'AQ==' }] }))
  const prepared = await service.resolve(
    request({ conversationId: first.conversationId, retryTurnId: first.turnId }),
    actor,
  )
  await service.purgeImages(1001, actor)
  const before = structuredClone([...store.rows.values()])
  await assert.rejects(
    service.reserve(prepared, execution(prepared, 'stale'), 'stale', actor),
    reason('preparation_changed'),
  )
  assert.deepEqual([...store.rows.values()], before)
  const translation = await service.resolve(
    request({ conversationId: first.conversationId, translateTurnId: first.turnId, targetLanguage: 'de' }),
    actor,
  )
  await service.editTurn(first.conversationId, first.turnId, 'edited', 'answer', actor)
  await assert.rejects(
    service.reserve(translation, execution(translation, 'translate'), 'translate', actor),
    reason('preparation_changed'),
  )
})

test('CAS retries unrelated rename but enforces implicit selected head and single running', async () => {
  const { service, store } = setup(),
    first = await complete(service, 'first')
  const prepared = await service.resolve(request({ conversationId: first.conversationId }), actor)
  store.beforeCas = async () => {
    await service.rename(first.conversationId, 'renamed', actor)
  }
  const reserved = await service.reserve(prepared, execution(prepared, 'second'), 'second', actor)
  assert.equal((await service.get(first.conversationId, actor)).summary.title, 'renamed')
  await assert.rejects(
    service.reserve(prepared, execution(prepared, 'blocked'), 'blocked', actor),
    reason('turn_running'),
  )
  await service.finish(reserved.handle, summary('two'), actor)
  const implicit = await service.resolve(request({ conversationId: first.conversationId }), actor)
  await service.selectTurn(first.conversationId, first.turnId, actor)
  await assert.rejects(
    service.reserve(implicit, execution(implicit, 'changed'), 'changed', actor),
    reason('preparation_changed'),
  )
  const explicit = await service.resolve(
    request({ conversationId: first.conversationId, replyToId: first.turnId }),
    actor,
  )
  await service.selectTurn(first.conversationId, 'second', actor)
  await service.reserve(explicit, execution(explicit, 'explicit'), 'explicit', actor)
})

test('edit CAS leaf restrictions, removal reparents and fixes supersedes, fork copies path and images', async () => {
  const { service } = setup(),
    first = await complete(service, 'first')
  await complete(service, 'retry', request({ conversationId: first.conversationId, retryTurnId: 'first' }))
  await complete(service, 'child', request({ conversationId: first.conversationId, replyToId: 'first' }))
  await complete(
    service,
    'translation',
    request({ conversationId: first.conversationId, translateTurnId: 'first', targetLanguage: 'fr' }),
  )
  await assert.rejects(
    service.editTurn(first.conversationId, 'first', 'text', 'answer', actor),
    reason('turn_has_dependents'),
  )
  await assert.rejects(service.editTurn(first.conversationId, 'child', 'text', 'wrong', actor), reason('text_changed'))
  await service.selectTurn(first.conversationId, 'first', actor)
  const removed = await service.removeTurn(first.conversationId, 'first', actor)
  assert.deepEqual(
    removed.turns.map((t) => t.id),
    ['retry', 'child'],
  )
  assert.equal(removed.turns[0]?.supersedesId, null)
  assert.equal(removed.turns[1]?.parentId, 'retry')
  assert.equal(removed.summary.selectedTurnId, 'child')
  const fork = await service.fork(first.conversationId, 'child', actor)
  assert.equal(fork.turns.length, 2)
  assert.notEqual(fork.summary.id, first.conversationId)
  assert.equal(fork.turns[1]?.parentId, fork.turns[0]?.id)
  assert.equal(fork.turns[0]?.cost?.amount, 0.1)
  const forkChild = fork.turns[1]
  assert.ok(forkChild)
  await service.editTurn(fork.summary.id, forkChild.id, 'edited fork', 'answer', actor)
  assert.equal((await service.get(first.conversationId, actor)).turns[1]?.text, 'answer')
})

test('generated image append persists; recover only touches matching running planned turn', async () => {
  const { service } = setup(),
    prepared = await service.resolve(request(), actor),
    plan = service.plan(prepared, 'run')
  await assert.rejects(
    service.recover(plan, actor),
    (error: unknown) => error instanceof KitFailure && error.code === 'not_found',
  )
  const reserved = await service.reserve(prepared, execution(prepared, 'run'), 'run', actor)
  const refs = await service.addGenerated(reserved.handle, [{ mime: 'image/png', data: 'AQ==' }], actor)
  assert.ok(refs[0]?.url?.endsWith('/generated/0'))
  await assert.rejects(service.recover({ ...plan, runId: 'wrong' }, actor))
  const recovered = await service.recover(plan, actor)
  assert.equal(recovered.turns[0]?.status, 'failed')
  assert.equal(recovered.turns[0]?.errorCode, 'interrupted')
  assert.equal(recovered.turns[0]?.generated.length, 1)
  await assert.rejects(service.recover(plan, actor), reason('turn_finished'))
})

test('HTTP validates fields and controlled image routes remain owner scoped', async () => {
  const { service } = setup(),
    first = await complete(service, 'first', request({ images: [{ mime: 'image/png', data: 'AQ==' }] }))
  const context = { actor, signal: new AbortController().signal, requestId: 'test', now: 1000 }
  const bad = new Request(`https://worker.example/api/v1/conversations/${first.conversationId}/rename`, {
    method: 'POST',
    body: JSON.stringify({ title: 'name', owner: 'other' }),
  })
  await assert.rejects(
    service.handle(bad, context),
    (error: unknown) => error instanceof KitFailure && error.code === 'bad_request',
  )
  const response = await service.handle(
    new Request(`https://worker.example/api/v1/conversations/${first.conversationId}/turns/first/images/input/0`),
    context,
  )
  assert.ok(response)
  assert.equal(response.headers.get('cache-control'), 'private, no-store')
  assert.deepEqual(new Uint8Array(await response.arrayBuffer()), new Uint8Array([1]))
  assert.equal(await service.handle(new Request('https://worker.example/api/v1/providers'), context), null)
})

test('malformed unknown stored document fails closed and actions reject invalid combinations', async () => {
  const { service, store } = setup()
  await store.compareAndSet(actor, COLLECTION, 'broken', null, { version: 1, summary: { id: 'broken' }, turns: [] })
  await assert.rejects(
    service.get('broken', actor),
    (error: unknown) => error instanceof KitFailure && error.code === 'incompatible_db',
  )
  const first = await complete(service, 'first')
  await assert.rejects(
    service.resolve(request({ conversationId: first.conversationId, retryTurnId: 'first', regenerate: true }), actor),
  )
  await assert.rejects(
    service.resolve(
      request({
        conversationId: first.conversationId,
        retryTurnId: 'first',
        images: [{ mime: 'image/png', data: 'AQ==' }],
      }),
      actor,
    ),
  )
})

test('scope listing and TTL sweep traverse more than one page', async () => {
  const { service, store } = setup(),
    first = await complete(service, 'seed', request({ images: [{ mime: 'image/png', data: 'AQ==' }] }))
  const source = await store.get(actor, COLLECTION, first.conversationId)
  assert.ok(source)
  for (let index = 0; index < 205; index++) {
    const document = parseDocument(source.value, source.key),
      id = `page-${String(index).padStart(3, '0')}`
    document.summary.id = id
    document.summary.scope = index === 204 ? 'wanted' : 'other'
    document.summary.updatedAt = index
    await store.compareAndSet(actor, COLLECTION, id, null, document)
  }
  assert.deepEqual(
    (await service.list('wanted', actor)).map((s) => s.id),
    ['page-204'],
  )
  assert.equal(await service.purgeImages(1001, actor), 206)
  assert.equal((await service.get('page-204', actor)).turns[0]?.images[0]?.url, null)
})

test('regenerate may presearch while explicit retry and translation cannot', async () => {
  const { service } = setup(),
    first = await complete(service, 'first')
  const regenerate = await service.resolve(
    request({ conversationId: first.conversationId, regenerate: true, webSearch: true }),
    actor,
  )
  assert.equal(regenerate.effectiveRequest.input, 'question')
  await assert.rejects(
    service.resolve(request({ conversationId: first.conversationId, retryTurnId: 'first', webSearch: true }), actor),
  )
  await assert.rejects(
    service.resolve(
      request({
        conversationId: first.conversationId,
        translateTurnId: 'first',
        targetLanguage: 'fr',
        webSearch: true,
      }),
      actor,
    ),
  )
})

test('fork copies input and generated metadata including expired rows without extending TTL', async () => {
  const { service, store, advance } = setup(),
    prepared = await service.resolve(request({ images: [{ mime: 'image/png', data: 'AQ==' }] }), actor)
  const reserved = await service.reserve(prepared, execution(prepared, 'first'), 'first', actor)
  await service.addGenerated(reserved.handle, [{ mime: 'image/png', data: 'Ag==' }], actor)
  await service.finish(reserved.handle, summary(), actor)
  await service.purgeImages(1001, actor)
  advance(9000)
  const fork = await service.fork(reserved.handle.conversationId, 'first', actor),
    copy = await store.get(actor, COLLECTION, fork.summary.id)
  assert.ok(copy)
  const turn = parseDocument(copy.value, copy.key).turns[0]
  assert.ok(turn)
  assert.deepEqual(turn.images, [{ mime: 'image/png', data: null, createdAt: 1000 }])
  assert.deepEqual(turn.generated, [{ mime: 'image/png', data: null, createdAt: 1000 }])
  assert.equal(fork.turns[0]?.images[0]?.url, null)
  assert.equal(fork.turns[0]?.generated[0]?.url, null)
})
