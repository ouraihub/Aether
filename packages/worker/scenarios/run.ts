import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { isContract } from '@ouraihub/aether-contracts'
import type { Approval, ModelSave, RunAccepted, RunEvent, RunRequest, RunSnapshot } from '@ouraihub/aether-contracts'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import { isRecord } from '@ouraihub/aether-core/guard'
import { validateArguments } from '@ouraihub/aether-core/schema'
import { DEFAULT_BUDGET, type ToolSpec } from '@ouraihub/aether-core/types'
import type { Actor } from '../src/context.js'
import { createRunCoordinator } from '../src/coordinator.js'
import { createCredentialPort } from '../src/credentials.js'
import type { WorkerHost } from '../src/host.js'
import { ProviderService } from '../src/providers/index.js'
import { WorkerRuntime } from '../src/runtime.js'
import { MemoryDocuments, MemoryState } from '../src/testing/fakes.js'

type Argument = string | { prefix: string; repeat: string; count: number }
interface ReplyParts {
  reasoning?: string[]
  text?: string[]
  calls?: { id: string; name: string; args: Record<string, Argument> }[]
}
interface ModelReply extends ReplyParts {
  gate?: string
  afterGate?: ReplyParts
}
interface Scenario {
  input: string
  maxRounds?: number
  tools: { name: string; approval: Approval }[]
  replies: ModelReply[]
}
interface Fixtures {
  owner: string
  model: { protocol: 'openai-chat'; modelId: string; contextWindow: number; maxOutput: number }
  tools: Record<
    string,
    { description: string; parameters: Record<string, unknown>; effect: ToolSpec['effect']; output: string }
  >
  scenarios: Record<string, Scenario>
}
interface Observation {
  accepted: RunAccepted[]
  events: RunEvent[]
  snapshots: { label: string; value: RunSnapshot }[]
  replies: { op: string; status: string; reason?: string }[]
  modelRequests: number
  dispatches: string[]
  marks: string[]
}

function assertReplyParts(value: unknown): asserts value is ReplyParts {
  assert.ok(isRecord(value), 'Model reply must be an object')
  for (const key of ['reasoning', 'text']) {
    const parts: unknown = value[key]
    assert.ok(
      parts === undefined || (Array.isArray(parts) && parts.every((part: unknown) => typeof part === 'string')),
      `Invalid reply ${key}`,
    )
  }
  if (value.calls === undefined) return
  assert.ok(Array.isArray(value.calls), 'Invalid reply calls')
  for (const call of value.calls) {
    assert.ok(
      isRecord(call) && typeof call.id === 'string' && typeof call.name === 'string' && isRecord(call.args),
      'Invalid model tool call',
    )
    for (const argument of Object.values(call.args)) {
      assert.ok(
        typeof argument === 'string' ||
          (isRecord(argument) &&
            typeof argument.prefix === 'string' &&
            typeof argument.repeat === 'string' &&
            typeof argument.count === 'number' &&
            Number.isSafeInteger(argument.count) &&
            argument.count >= 0),
        'Invalid expanded argument',
      )
    }
  }
}

function assertFixtures(value: unknown): asserts value is Fixtures {
  assert.ok(isRecord(value) && typeof value.owner === 'string' && value.owner.length > 0, 'Invalid fixture owner')
  assert.ok(
    isRecord(value.model) && value.model.protocol === 'openai-chat' && typeof value.model.modelId === 'string',
    'Invalid fixture model',
  )
  for (const key of ['contextWindow', 'maxOutput']) {
    const limit: unknown = value.model[key]
    assert.ok(typeof limit === 'number' && Number.isSafeInteger(limit) && limit > 0, `Invalid model ${key}`)
  }
  assert.ok(isRecord(value.tools), 'Invalid fixture tools')
  for (const tool of Object.values(value.tools)) {
    assert.ok(
      isRecord(tool) &&
        typeof tool.description === 'string' &&
        isRecord(tool.parameters) &&
        ['read', 'write', 'other'].includes(String(tool.effect)) &&
        typeof tool.output === 'string',
      'Invalid fixture tool',
    )
  }
  assert.ok(isRecord(value.scenarios), 'Invalid fixture scenarios')
  for (const scenario of Object.values(value.scenarios)) {
    assert.ok(
      isRecord(scenario) &&
        typeof scenario.input === 'string' &&
        Array.isArray(scenario.tools) &&
        Array.isArray(scenario.replies),
      'Invalid scenario',
    )
    assert.ok(
      scenario.maxRounds === undefined ||
        (typeof scenario.maxRounds === 'number' && Number.isSafeInteger(scenario.maxRounds) && scenario.maxRounds > 0),
      'Invalid maxRounds',
    )
    for (const tool of scenario.tools) {
      assert.ok(
        isRecord(tool) && typeof tool.name === 'string' && ['auto', 'ask', 'deny'].includes(String(tool.approval)),
        'Invalid scenario tool',
      )
    }
    for (const reply of scenario.replies) {
      assertReplyParts(reply)
      assert.ok(isRecord(reply), 'Invalid reply')
      assert.ok(reply.gate === undefined || typeof reply.gate === 'string', 'Invalid model gate')
      if (reply.afterGate !== undefined) assertReplyParts(reply.afterGate)
    }
  }
}

async function barrier<T>(label: string, promise: Promise<T>): Promise<T> {
  const timeout = Promise.withResolvers<never>()
  const timer = setTimeout(() => timeout.reject(new Error(`Missing barrier: ${label}`)), 10000)
  try {
    return await Promise.race([promise, timeout.promise])
  } finally {
    clearTimeout(timer)
  }
}

function deltas(parts: ReplyParts): Record<string, unknown>[] {
  return [
    ...(parts.reasoning ?? []).map((reasoning_content) => ({ reasoning_content })),
    ...(parts.text ?? []).map((content) => ({ content })),
    ...(parts.calls ?? []).map((call, index) => ({
      tool_calls: [
        {
          index,
          id: call.id,
          type: 'function',
          function: {
            name: call.name,
            arguments: JSON.stringify(
              Object.fromEntries(
                Object.entries(call.args).map(([key, value]) => [
                  key,
                  typeof value === 'string' ? value : value.prefix + value.repeat.repeat(value.count),
                ]),
              ),
            ),
          },
        },
      ],
    })),
  ]
}

async function run(id: string, scenario: Scenario, fixtures: Fixtures): Promise<Observation> {
  const observation: Observation = {
    accepted: [],
    events: [],
    snapshots: [],
    replies: [],
    modelRequests: 0,
    dispatches: [],
    marks: [],
  }
  const gates = new Map<string, PromiseWithResolvers<void>>()
  for (const reply of scenario.replies) {
    if (reply.gate && !gates.has(reply.gate)) gates.set(reply.gate, Promise.withResolvers<void>())
  }
  const release = (name: string): void => {
    const gate = gates.get(name)
    if (!gate) throw new Error(`Unknown model gate: ${name}`)
    gate.resolve()
    observation.marks.push(`${name}-released`)
  }
  const outbound: OutboundHttp = {
    async fetch(url, init) {
      if (url !== 'https://scenario.example/v1/chat/completions' || init.method !== 'POST')
        throw new Error(`Unexpected model request: ${url}`)
      const body: unknown = JSON.parse(String(init.body))
      if (
        !isRecord(body) ||
        body.model !== fixtures.model.modelId ||
        body.stream !== true ||
        !Array.isArray(body.messages)
      )
        throw new Error('Invalid encoded model request')
      const reply = scenario.replies[observation.modelRequests++]
      if (!reply) throw new Error('Controlled model ran out of replies')
      const frames = [...deltas(reply), ...deltas(reply.afterGate ?? {})]
      const encoder = new TextEncoder()
      let index = 0
      let closed = false
      let controller: ReadableStreamDefaultController<Uint8Array>
      const abort = (): void => {
        if (!closed) {
          closed = true
          init.signal?.removeEventListener('abort', abort)
          controller.error(init.signal?.reason ?? new Error('Model request aborted'))
        }
      }
      const detach = (): void => init.signal?.removeEventListener('abort', abort)
      return new Response(
        new ReadableStream<Uint8Array>({
          start(value) {
            controller = value
            init.signal?.addEventListener('abort', abort, { once: true })
            if (init.signal?.aborted) abort()
          },
          async pull() {
            if (reply.gate && index === 1) await barrier(`model gate ${reply.gate}`, gates.get(reply.gate)!.promise)
            if (closed) return
            if (index < frames.length) {
              controller.enqueue(
                encoder.encode(
                  `data: ${JSON.stringify({ choices: [{ index: 0, delta: frames[index++], finish_reason: null }] })}\n\n`,
                ),
              )
            } else {
              const calls = [...(reply.calls ?? []), ...(reply.afterGate?.calls ?? [])]
              controller.enqueue(
                encoder.encode(
                  `data: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: calls.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`,
                ),
              )
              closed = true
              detach()
              controller.close()
            }
          },
          cancel() {
            closed = true
            detach()
          },
        }),
        { headers: { 'Content-Type': 'text/event-stream' } },
      )
    },
  }
  const actor: Actor = {
    subjectId: fixtures.owner,
    applicationId: 'scenario',
    kind: 'user',
    tenantId: null,
    sessionId: 'scenario',
    scopes: ['*'],
  }
  const documents = new MemoryDocuments()
  const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'])
  const credentials = createCredentialPort({ currentKeyId: 'scenario', keys: new Map([['scenario', key]]) })
  const providers = new ProviderService({
    documents,
    credentials,
    outbound,
    async authorizeReveal() {
      throw new Error('Credential reveal is not part of a scenario')
    },
  })
  const model: ModelSave = {
    id: fixtures.model.modelId,
    modelType: 'chat',
    typeSource: 'user',
    contextWindow: fixtures.model.contextWindow,
    maxOutput: fixtures.model.maxOutput,
    canVision: false,
    supportsTools: true,
    nativeWebSearch: false,
    reasoning: null,
    name: null,
    groupName: null,
    pricing: null,
  }
  const saved = await providers.save(actor, {
    name: 'Scenario',
    endpoints: [
      { slug: 'scenario', protocol: fixtures.model.protocol, baseUrl: 'https://scenario.example/v1', models: [model] },
    ],
  })
  const endpoint = saved[0]?.endpoints[0]
  if (!endpoint) throw new Error('Scenario model endpoint was not saved')
  await providers.saveKey(actor, { endpointId: endpoint.id, key: 'scenario-token' })
  const host: WorkerHost = {
    async selectModel() {
      return { endpointId: endpoint.id, modelId: model.id }
    },
    async systemPrompt() {
      return null
    },
    async context() {
      return null
    },
    async tools() {
      return scenario.tools.map(({ name, approval }): ToolSpec => {
        const tool = fixtures.tools[name]
        if (!tool) throw new Error(`Unknown fixture tool: ${name}`)
        return {
          name,
          description: tool.description,
          parameters: tool.parameters,
          effect: tool.effect,
          approval,
          source: { kind: 'host' },
        }
      })
    },
    permissionModes() {
      return ['default']
    },
    authorize(_call, authorization) {
      return authorization.proposed
    },
    async callTool(call, signal) {
      signal.throwIfAborted()
      const tool = fixtures.tools[call.name]
      if (!tool) throw new Error(`Unknown dispatched tool: ${call.name}`)
      observation.dispatches.push(call.id)
      return { kind: 'ok', text: tool.output }
    },
    async runFinished() {},
  }
  const runtime = new WorkerRuntime({
    host,
    providers,
    conversations: null,
    outbound,
    validateArguments,
    budget: {
      ...DEFAULT_BUDGET,
      contextWindow: fixtures.model.contextWindow,
      maxOutput: fixtures.model.maxOutput,
      ...(scenario.maxRounds === undefined ? {} : { maxRounds: scenario.maxRounds }),
    },
  })
  const state = new MemoryState()
  const Coordinator = createRunCoordinator(() => runtime)
  const coordinator = new Coordinator(state, undefined)
  const actorHeader = Buffer.from(JSON.stringify(actor)).toString('base64')
  const request = (path: string, body?: unknown): Request =>
    new Request(`https://coordinator${path}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: { 'x-aether-actor': actorHeader, 'Content-Type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    })
  const input: RunRequest = { purpose: 'chat', input: scenario.input, skills: [], regenerate: false, language: 'en' }
  const acceptedResponse = await barrier(
    'backend-accepted',
    coordinator.fetch(
      request('/accept', {
        runId: `scenario-${id}`,
        actor,
        request: input,
      }),
    ),
  )
  if (acceptedResponse.status !== 202) throw new Error(`Acceptance failed: ${await acceptedResponse.text()}`)
  const accepted: unknown = await acceptedResponse.json()
  assert.ok(isContract('RunAccepted', accepted), 'Invalid RunAccepted response')
  const normalizedAccepted: RunAccepted = {
    ...accepted,
    runId: '<run>',
    conversationId: accepted.conversationId === null ? null : '<conversation>',
    turnId: accepted.turnId === null ? null : '<turn>',
  }
  if (id !== 'acceptance-response-race') observation.accepted.push(normalizedAccepted)
  const response = await barrier('receiver-subscribed', coordinator.fetch(request('/events')))
  if (!response.ok || !response.body) throw new Error(`Subscription failed: ${await response.text()}`)
  const reader = response.body.getReader()
  const end = Promise.withResolvers<void>()
  const first = Promise.withResolvers<void>()
  const approval = Promise.withResolvers<Extract<RunEvent, { kind: 'approval_required' }>>()
  const pump = (async () => {
    const decoder = new TextDecoder()
    let pending = ''
    for (;;) {
      const next = await reader.read()
      if (next.done) break
      pending += decoder.decode(next.value, { stream: true })
      let boundary: number
      while ((boundary = pending.indexOf('\n\n')) !== -1) {
        const frame = pending.slice(0, boundary)
        pending = pending.slice(boundary + 2)
        const data = frame.split('\n').find((line) => line.startsWith('data: '))
        if (!data) continue
        const event: unknown = JSON.parse(data.slice(6))
        assert.ok(isContract('RunEvent', event), 'Invalid RunEvent frame')
        observation.events.push(event)
        if (event.kind === 'end') end.resolve()
        if (event.kind === 'text_delta' && event.text === scenario.replies[0]?.text?.[0]) first.resolve()
        if (event.kind === 'approval_required') approval.resolve(event)
      }
    }
  })()
  // Propagate receiver failures to every outstanding event barrier, rather than timing them out.
  void pump.catch((error: unknown) => {
    end.reject(error)
    first.reject(error)
    approval.reject(error)
  })
  // Unused event barriers must not produce unhandled rejections when another barrier reports a failure.
  void end.promise.catch(() => undefined)
  void first.promise.catch(() => undefined)
  void approval.promise.catch(() => undefined)

  if (id === 'acceptance-response-race') {
    observation.marks.push('backend-accepted')
    release('model')
    await barrier('end-received', end.promise)
    observation.marks.push('end-received')
    observation.accepted.push(normalizedAccepted)
    observation.marks.push('accepted-delivered')
  } else if (id === 'tool-approval-allow') {
    const event = await barrier('approval-required', approval.promise)
    observation.marks.push('approval-required', `dispatches-before-approval:${observation.dispatches.length}`)
    for (const op of ['approve', 'approve-again']) {
      const reply = await barrier(
        op,
        coordinator.fetch(request('/approval', { callId: event.callId, decision: 'allow' })),
      )
      if (reply.ok) observation.replies.push({ op, status: 'ok' })
      else {
        const error: unknown = await reply.json()
        assert.ok(isContract('KitError', error), 'Invalid approval error response')
        const reason = error.details?.reason
        observation.replies.push({ op, status: error.code, ...(typeof reason === 'string' ? { reason } : {}) })
      }
    }
    await barrier('end-received', end.promise)
    observation.marks.push('end-received')
  } else if (id === 'receiver-send-failure') {
    await barrier('first-delta', first.promise)
    observation.marks.push('first-delta')
    const cancellation = reader.cancel()
    observation.marks.push('receiver-closed')
    release('model')
    await barrier('terminal', cancellation)
    await barrier('terminal', state.drain())
    observation.marks.push('terminal')
  } else if (id === 'round-exhaustion') {
    await barrier('end-received', end.promise)
    observation.marks.push('end-received')
  } else {
    throw new Error(`No executable scenario procedure: ${id}`)
  }
  await barrier('receiver-finished', pump)
  await barrier('terminal', state.drain())
  const snapshotResponse = await barrier('snapshot-final', coordinator.fetch(request('/snapshot')))
  if (!snapshotResponse.ok) throw new Error(`Snapshot failed: ${await snapshotResponse.text()}`)
  const snapshot: unknown = await snapshotResponse.json()
  assert.ok(isContract('RunSnapshot', snapshot), 'Invalid RunSnapshot response')
  observation.snapshots.push({
    label: 'final',
    value: {
      ...snapshot,
      runId: '<run>',
      conversationId: snapshot.conversationId === null ? null : '<conversation>',
      turnId: snapshot.turnId === null ? null : '<turn>',
    },
  })
  return observation
}

const fixtures: unknown = JSON.parse(
  await readFile(new URL('../../../contracts/scenario-fixtures.json', import.meta.url), 'utf8'),
)
assertFixtures(fixtures)
const scenarios: Record<string, Observation> = {}
for (const [id, scenario] of Object.entries(fixtures.scenarios)) scenarios[id] = await run(id, scenario, fixtures)
process.stdout.write(`${JSON.stringify({ backend: 'worker', scenarios })}\n`)
