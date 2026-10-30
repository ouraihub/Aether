import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { ModelRef, ModelSave, RunEvent, RunRequest } from '@ouraihub/aether-contracts'
import { KitFailure } from '@ouraihub/aether-core/error'
import type { ArgumentsValidator, OutboundHttp } from '@ouraihub/aether-core/ports'
import type { ToolExecution, ToolSpec } from '@ouraihub/aether-core/types'
import type { Actor, WorkerContext } from './context.js'
import { ConversationService } from './conversations/index.js'
import { createCredentialPort } from './credentials.js'
import type { WorkerHost } from './host.js'
import { ProviderService } from './providers/index.js'
import { WorkerRuntime } from './runtime.js'
import { MemoryDocuments } from './testing/fakes.js'

const actor: Actor = {
  subjectId: 'subject',
  applicationId: 'app',
  kind: 'user',
  tenantId: null,
  sessionId: 'session',
  scopes: ['*'],
}
const request: RunRequest = {
  purpose: 'chat',
  input: 'Original authoritative question',
  skills: [],
  regenerate: false,
  language: 'en',
}
const model: ModelSave = {
  id: 'test-model',
  modelType: 'chat',
  typeSource: 'user',
  contextWindow: 32000,
  maxOutput: 1024,
  canVision: false,
  supportsTools: true,
  nativeWebSearch: false,
  reasoning: null,
  name: null,
  groupName: null,
  pricing: null,
}
const hostTool: ToolSpec = {
  name: 'read_item',
  description: 'Read one item',
  parameters: { type: 'object', properties: {}, additionalProperties: false },
  effect: 'read',
  approval: 'ask',
  source: { kind: 'host' },
}
// This fixture schema has precisely one supported shape; unknown schemas and extra args fail closed.
const validateArguments: ArgumentsValidator = (schema, args) =>
  schema.type === 'object' && schema.additionalProperties === false && Object.keys(args).length === 0
const allowed = {
  async wait() {
    return true
  },
}
function response(tool = false): Response {
  const delta = tool
    ? { tool_calls: [{ index: 0, id: 'call-1', type: 'function', function: { name: 'read_item', arguments: '{}' } }] }
    : { content: 'Visible answer' }
  return new Response(
    `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: null }] })}\n\ndata: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: tool ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`,
    { headers: { 'Content-Type': 'text/event-stream' } },
  )
}
async function setup(
  overrides: Partial<WorkerHost> = {},
  history = true,
  outbound: OutboundHttp = {
    async fetch() {
      return response()
    },
  },
) {
  const documents = new MemoryDocuments()
  const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'])
  const credentials = createCredentialPort({ currentKeyId: 'key', keys: new Map([['key', key]]) })
  const providers = new ProviderService({
    documents,
    credentials,
    outbound,
    async authorizeReveal() {
      throw new KitFailure('forbidden', 'Not allowed')
    },
  })
  const saved = await providers.save(actor, {
    name: 'Test',
    endpoints: [{ slug: 'test', protocol: 'openai-chat', baseUrl: 'https://models.example', models: [model] }],
  })
  const endpoint = saved[0]?.endpoints[0]
  assert.ok(endpoint)
  await providers.saveKey(actor, { endpointId: endpoint.id, key: 'secret-token' })
  const reference: ModelRef = { endpointId: endpoint.id, modelId: model.id }
  const observed: RunRequest[] = []
  let cleanups = 0,
    toolCalls = 0
  const host: WorkerHost = {
    async selectModel(effective) {
      observed.push(structuredClone(effective))
      return effective.model ?? reference
    },
    async systemPrompt(effective) {
      observed.push(structuredClone(effective))
      return 'System instructions'
    },
    async context(effective) {
      observed.push(structuredClone(effective))
      return null
    },
    async tools(effective) {
      observed.push(structuredClone(effective))
      return []
    },
    permissionModes(effective) {
      observed.push(structuredClone(effective))
      return ['default', 'plan', 'accept_edits']
    },
    authorize(_call, authorization) {
      return authorization.proposed
    },
    async callTool() {
      toolCalls++
      return { kind: 'ok', text: 'Read item' }
    },
    async runFinished() {
      cleanups++
    },
    ...overrides,
  }
  const conversations = new ConversationService({ store: documents })
  const runtime = new WorkerRuntime({
    host,
    providers,
    conversations: history ? conversations : null,
    outbound,
    validateArguments,
  })
  const post = new AbortController()
  const context: WorkerContext = { actor, signal: post.signal, requestId: 'request', now: Date.now() }
  return {
    runtime,
    conversations,
    documents,
    reference,
    observed,
    post,
    context,
    cleanups: () => cleanups,
    toolCalls: () => toolCalls,
  }
}

test('prepare failures leave no reservation and clean host exactly once', async () => {
  const state = await setup({
    async tools() {
      throw new Error('host-secret')
    },
  })
  let planned = false
  await assert.rejects(
    state.runtime.prepare('rejected', request, state.context, async () => {
      planned = true
    }),
  )
  assert.equal(planned, false)
  assert.deepEqual(await state.conversations.list(null, actor), [])
  assert.equal(state.cleanups(), 1)
})
test('onPlan fails before CAS without residual', async () => {
  const state = await setup()
  await assert.rejects(
    state.runtime.prepare('plan-fails', request, state.context, async (handle) => {
      assert.ok(handle)
      assert.deepEqual(await state.conversations.list(null, actor), [])
      throw new Error('Metadata persistence failed')
    }),
  )
  assert.equal(state.cleanups(), 1)
  assert.deepEqual(await state.conversations.list(null, actor), [])
})
test('changed authoritative history after onPlan rejects reservation without a candidate', async () => {
  const state = await setup()
  const first = await state.runtime.prepare('history-first', request, state.context, async () => {})
  await state.runtime.execute(first, new AbortController().signal, () => {}, allowed)
  const conversationId = first.accepted.conversationId
  assert.ok(conversationId)
  await assert.rejects(
    state.runtime.prepare('changed-history', { ...request, conversationId }, state.context, async () => {
      await state.conversations.editTurn(conversationId, 'history-first', 'Changed answer', 'Visible answer', actor)
    }),
    (error) =>
      error instanceof KitFailure && error.code === 'conflict' && error.details?.reason === 'preparation_changed',
  )
  const conversation = await state.conversations.get(conversationId, actor)
  assert.equal(conversation.turns.length, 1)
  assert.equal(conversation.turns[0]?.text, 'Changed answer')
  assert.equal(state.cleanups(), 2)
})
test('source input and source model precede every host hook, secrets never project', async () => {
  const state = await setup()
  const first = await state.runtime.prepare('first', request, state.context, async () => {})
  const firstSnapshot = await state.runtime.execute(first, new AbortController().signal, () => {}, allowed)
  assert.equal(firstSnapshot.status, 'completed')
  state.observed.length = 0
  const retry = await state.runtime.prepare(
    'retry',
    { ...request, input: '', conversationId: first.accepted.conversationId, retryTurnId: 'first' },
    state.context,
    async () => {},
  )
  assert.ok(state.observed.length >= 5)
  for (const effective of state.observed) {
    assert.equal(effective.input, request.input)
    assert.deepEqual(effective.model, state.reference)
  }
  assert.equal(JSON.stringify(retry).includes('secret-token'), false)
  assert.equal(JSON.stringify(retry).includes(request.input), false)
  const snapshot = await state.runtime.abandon(retry, 'cancelled')
  assert.equal(snapshot.status, 'cancelled')
  assert.deepEqual(state.runtime.snapshot(retry), snapshot)
  assert.equal(JSON.stringify(snapshot).includes('secret-token'), false)
  assert.equal(state.cleanups(), 2)
  await state.runtime.abandon(retry, 'failed')
  assert.equal(state.cleanups(), 2)
})
test('saving fails to one safe failed End, never a completed End', async () => {
  const state = await setup()
  const prepared = await state.runtime.prepare('save-fails', request, state.context, async () => {})
  state.documents.failConversations = true
  const events: RunEvent[] = []
  const snapshot = await state.runtime.execute(
    prepared,
    new AbortController().signal,
    (event) => events.push(event),
    allowed,
  )
  assert.equal(snapshot.status, 'failed')
  assert.equal(state.runtime.snapshot(prepared).status, 'failed')
  assert.equal(state.cleanups(), 1)
  assert.deepEqual(
    events.filter((event) => event.kind === 'end').map((event) => event.status),
    ['failed'],
  )
  assert.equal(events.at(-1)?.kind, 'end')
  assert.equal(JSON.stringify(snapshot).includes('secret-token'), false)
  assert.equal(JSON.stringify(events).includes('secret-token'), false)
})
test('POST abort after acceptance does not cancel runtime execution; no-history rejects actions', async () => {
  const state = await setup({}, false)
  const prepared = await state.runtime.prepare('stateless', request, state.context, async (handle) => {
    assert.equal(handle, null)
  })
  state.post.abort()
  const snapshot = await state.runtime.execute(prepared, new AbortController().signal, () => {}, allowed)
  assert.equal(snapshot.status, 'completed')
  assert.equal(snapshot.conversationId, null)
  assert.deepEqual(await state.conversations.list(null, actor), [])
  const next = await setup({}, false)
  await assert.rejects(
    next.runtime.prepare('unsupported', { ...request, conversationId: 'existing' }, next.context, async () => {}),
    (error) => error instanceof KitFailure && error.details?.reason === 'history_disabled',
  )
  assert.equal(next.cleanups(), 1)
})
test('approval cancellation never dispatches tool and cleanup follows cancelled persistence', async () => {
  const state = await setup(
    {
      async tools() {
        return [hostTool]
      },
    },
    true,
    {
      async fetch() {
        return response(true)
      },
    },
  )
  const prepared = await state.runtime.prepare('approval-cancel', request, state.context, async () => {})
  const cancellation = new AbortController(),
    events: RunEvent[] = []
  const snapshot = await state.runtime.execute(
    prepared,
    cancellation.signal,
    (event) => {
      events.push(event)
      if (event.kind === 'approval_required') cancellation.abort()
    },
    {
      async wait(_runId, _call, signal) {
        if (signal.aborted) return false
        const pending = Promise.withResolvers<boolean>()
        signal.addEventListener('abort', () => pending.resolve(false), { once: true })
        return pending.promise
      },
    },
  )
  assert.equal(snapshot.status, 'cancelled')
  assert.equal(state.toolCalls(), 0)
  assert.equal(state.cleanups(), 1)
  assert.deepEqual(
    events.filter((event) => event.kind === 'end').map((event) => event.status),
    ['cancelled'],
  )
  const conversationId = prepared.accepted.conversationId
  assert.ok(conversationId)
  assert.equal((await state.conversations.get(conversationId, actor)).turns[0]?.status, 'cancelled')
})
test('cancelled in-flight host work settles before finish and cleanup', async () => {
  const started = Promise.withResolvers<void>(),
    released = Promise.withResolvers<ToolExecution>()
  const cancellation = new AbortController()
  const state = await setup(
    {
      async tools() {
        return [{ ...hostTool, approval: 'auto' }]
      },
      async callTool() {
        started.resolve()
        return released.promise
      },
    },
    true,
    {
      async fetch() {
        return response(true)
      },
    },
  )
  const prepared = await state.runtime.prepare('inflight', request, state.context, async () => {})
  const execution = state.runtime.execute(prepared, cancellation.signal, () => {}, allowed)
  await started.promise
  cancellation.abort()
  assert.equal(state.cleanups(), 0)
  released.resolve({ kind: 'ok', text: 'Confirmed after cancellation' })
  const snapshot = await execution
  assert.equal(snapshot.status, 'cancelled')
  assert.equal(state.cleanups(), 1)
})
test('abort during host preparation prevents reservation and cleans after hook settles', async () => {
  const entered = Promise.withResolvers<void>(),
    release = Promise.withResolvers<string | null>()
  const state = await setup({
    async systemPrompt() {
      entered.resolve()
      return release.promise
    },
  })
  const preparation = state.runtime.prepare('preparation-abort', request, state.context, async () => {})
  await entered.promise
  state.post.abort()
  assert.equal(state.cleanups(), 0)
  release.resolve(null)
  await assert.rejects(preparation, (error) => error instanceof KitFailure && error.code === 'cancelled')
  assert.equal(state.cleanups(), 1)
  assert.deepEqual(await state.conversations.list(null, actor), [])
})
