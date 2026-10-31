import assert from 'node:assert/strict'
import test from 'node:test'
import {
  isContract,
  type RunRequest,
  type RunSnapshot,
} from '@ouraihub/aether-contracts'
import { KitFailure } from '@ouraihub/aether-core'
import type { DurableObjectState, DurableObjectStorage } from './cloudflare.js'
import type { Actor } from './context.js'
import type { AcceptedHandle } from './conversations/index.js'
import type { PreparedExecution, WorkerRuntime } from './runtime.js'
import { createRunCoordinator } from './coordinator.js'
import { createAetherWorkerApp } from './app.js'

type Runtime = Pick<
  WorkerRuntime,
  'prepare' | 'execute' | 'abandon' | 'recover'
>
interface StoredRun {
  phase: string
  handle: AcceptedHandle | null
  snapshot: RunSnapshot
  pending: { callId: string; revision: number } | null
}
const actor: Actor = {
  subjectId: 'owner',
  applicationId: 'app',
  tenantId: null,
  sessionId: 'session',
  kind: 'user',
  scopes: [],
}
const input: RunRequest = {
  purpose: 'chat',
  input: 'private prompt',
  skills: [],
  regenerate: false,
  language: 'en',
}
const handle: AcceptedHandle = {
  runId: 'run-1',
  conversationId: 'conversation-1',
  turnId: 'turn-1',
  reservationId: 'reservation-1',
}
const prepared: PreparedExecution = {
  accepted: {
    runId: handle.runId,
    conversationId: handle.conversationId,
    turnId: handle.turnId,
  },
  handle,
}
const snapshot = (status: RunSnapshot['status']): RunSnapshot => ({
  ...prepared.accepted,
  status,
  cancelRequested: status === 'cancelled',
})

class Storage implements DurableObjectStorage {
  readonly records = new Map<string, unknown>()
  readonly writes: unknown[] = []
  alarm: number | null = null
  async get<T = unknown>(key: string): Promise<T | undefined> {
    return structuredClone(this.records.get(key)) as T | undefined
  }
  async put<T>(key: string, value: T): Promise<void> {
    this.records.set(key, structuredClone(value))
    this.writes.push(structuredClone(value))
  }
  async delete(key: string): Promise<boolean> {
    return this.records.delete(key)
  }
  async setAlarm(value: number | Date): Promise<void> {
    this.alarm = Number(value)
  }
  async getAlarm(): Promise<number | null> {
    return this.alarm
  }
  async deleteAlarm(): Promise<void> {
    this.alarm = null
  }
}
class State implements DurableObjectState {
  readonly storage: Storage
  readonly tasks = new Set<Promise<unknown>>()
  constructor(storage = new Storage()) {
    this.storage = storage
  }
  blockConcurrencyWhile<T>(callback: () => Promise<T>): Promise<T> {
    return callback()
  }
  waitUntil(promise: Promise<unknown>): void {
    this.tasks.add(promise)
    void promise
      .finally(() => this.tasks.delete(promise))
      .catch(() => undefined)
  }
  async drain(): Promise<void> {
    while (this.tasks.size) await Promise.all([...this.tasks])
  }
}
function internal(
  path: string,
  current = actor,
  body?: unknown,
  signal?: AbortSignal,
): Request {
  const encoded = btoa(
    String.fromCharCode(...new TextEncoder().encode(JSON.stringify(current))),
  )
  return new Request(`https://coordinator${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'x-aether-actor': encoded, 'Content-Type': 'application/json' },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    ...(signal ? { signal } : {}),
  })
}
function fixture(overrides: Partial<Runtime> = {}, storage?: Storage) {
  const state = new State(storage)
  const runtime: Runtime = {
    async prepare(_id, _request, _context, onPlan) {
      await onPlan(handle)
      return prepared
    },
    async execute(_prepared, _signal, onEvent) {
      onEvent({ kind: 'text_delta', text: 'hello' })
      onEvent({ kind: 'end', status: 'completed' })
      return snapshot('completed')
    },
    async abandon() {
      return snapshot('cancelled')
    },
    async recover() {},
    ...overrides,
  }
  const Coordinator = createRunCoordinator(() => runtime)
  const coordinator = new Coordinator(state, undefined)
  const accept = (current = actor, signal?: AbortSignal) =>
    coordinator.fetch(
      new Request('https://coordinator/accept', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          runId: handle.runId,
          actor: current,
          request: input,
        }),
        ...(signal ? { signal } : {}),
      }),
    )
  return { coordinator, state, runtime, accept }
}

test('accept persists creating and planned handle before reservation; only the first subscriber starts execution', async () => {
  const execution = Promise.withResolvers<RunSnapshot>()
  let starts = 0
  const fixtureValue = fixture({
    async prepare(_id, _request, _context, onPlan) {
      const creating = await state.storage.get<StoredRun>('run')
      assert.ok(creating)
      assert.equal(creating?.phase, 'creating')
      assert.equal(creating.handle, null)
      await onPlan(handle)
      const planned = await state.storage.get<StoredRun>('run')
      assert.ok(planned)
      assert.equal(planned?.phase, 'creating')
      assert.deepEqual(planned.handle, handle)
      return prepared
    },
    async execute(_prepared, _signal, emit) {
      starts++
      emit({ kind: 'round_start', round: 1 })
      return execution.promise
    },
  })
  const state = fixtureValue.state
  const { coordinator, accept } = fixtureValue
  const accepts = await Promise.all([
    accept(),
    accept({ ...actor, subjectId: 'racing-owner' }),
  ])
  assert.deepEqual(
    accepts.map((response) => response.status).sort(),
    [202, 409],
  )
  assert.equal(starts, 0)
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'accepted')
  const streams = await Promise.all([
    coordinator.fetch(internal('/events')),
    coordinator.fetch(internal('/events')),
  ])
  assert.deepEqual(
    streams.map((response) => response.status).sort(),
    [200, 409],
  )
  assert.equal(starts, 1)
  execution.resolve(snapshot('completed'))
  const stream = streams.find((response) => response.status === 200)
  assert.ok(stream)
  const body = await stream.text()
  assert.equal(body.match(/"kind":"end"/gu)?.length, 1)
  assert.ok(body.endsWith('"status":"completed"}\n\n'))
  const finished = await state.storage.get<StoredRun>('run')
  assert.ok(finished)
  assert.equal(finished?.phase, 'completed')
  assert.equal(finished.snapshot.status, 'completed')
  await state.drain()
})

test('application tenant subject and session are all exact owner boundaries; foreign cancel is an acknowledgement', async () => {
  const { coordinator, accept, state } = fixture()
  assert.equal((await accept()).status, 202)
  for (const foreign of [
    { ...actor, applicationId: 'other' },
    { ...actor, tenantId: 'other' },
    { ...actor, subjectId: 'other' },
    { ...actor, sessionId: null },
  ]) {
    assert.equal(
      (await coordinator.fetch(internal('/snapshot', foreign))).status,
      404,
    )
    assert.equal(
      (await coordinator.fetch(internal('/events', foreign))).status,
      404,
    )
    assert.equal(
      (
        await coordinator.fetch(
          internal('/approval', foreign, { callId: 'call', decision: 'allow' }),
        )
      ).status,
      404,
    )
    assert.deepEqual(
      await (await coordinator.fetch(internal('/cancel', foreign, {}))).json(),
      { ok: true },
    )
  }
  const accepted = await state.storage.get<StoredRun>('run')
  assert.ok(accepted)
  assert.equal(accepted?.phase, 'accepted')
  assert.equal(accepted.snapshot.cancelRequested, false)
  assert.deepEqual(
    await (await coordinator.fetch(internal('/cancel', actor, {}))).json(),
    { ok: true },
  )
  await state.drain()
})

test('accepted alarm cancels without executing; terminal subscription contains only End and replay is rejected', async (context) => {
  context.mock.timers.enable({ apis: ['Date'], now: 1000 })
  let starts = 0,
    abandons = 0
  const { coordinator, accept, state } = fixture({
    async execute() {
      starts++
      return snapshot('completed')
    },
    async abandon() {
      abandons++
      return snapshot('cancelled')
    },
  })
  await accept()
  assert.equal(state.storage.alarm, 31000)
  context.mock.timers.setTime(30999)
  await coordinator.alarm()
  assert.equal(abandons, 0)
  context.mock.timers.setTime(31000)
  await coordinator.alarm()
  assert.equal(starts, 0)
  assert.equal(abandons, 1)
  const body = await (await coordinator.fetch(internal('/events'))).text()
  assert.match(
    body,
    /^event: run_event\nid: 1\ndata: \{"kind":"end","status":"cancelled"\}\n\n$/u,
  )
  const replay = internal('/events')
  replay.headers.set('Last-Event-ID', '1')
  const rejected = await coordinator.fetch(replay)
  assert.equal(rejected.status, 409)
  assert.deepEqual(await rejected.json(), {
    code: 'conflict',
    message: 'Run state conflict',
    details: { reason: 'event_replay_unavailable' },
  })
  assert.equal(state.storage.alarm, null)
  await state.drain()
})

test('approval is durable before delivery and a decision before approver.wait is retained without persisting args', async () => {
  const beforeWait = Promise.withResolvers<void>()
  const decision = Promise.withResolvers<boolean>()
  const { coordinator, state, accept } = fixture({
    async execute(_prepared, signal, emit, approver) {
      emit({
        kind: 'approval_required',
        callId: 'call-1',
        name: 'write',
        argsPreview: 'private args preview',
      })
      await beforeWait.promise
      const allowed = await approver.wait(
        handle.runId,
        { id: 'call-1', name: 'write', args: { secret: 'private args' } },
        signal,
      )
      decision.resolve(allowed)
      emit({
        kind: 'tool_result',
        callId: 'call-1',
        outcome: allowed ? 'ok' : 'denied',
        summary: 'done',
      })
      return snapshot('completed')
    },
  })
  await accept()
  const response = await coordinator.fetch(internal('/events'))
  const reader = response.body?.getReader()
  assert.ok(reader)
  const first = await reader.read()
  assert.ok(new TextDecoder().decode(first.value).includes('approval_required'))
  const pending = (await state.storage.get<StoredRun>('run'))?.pending
  assert.ok(pending)
  assert.equal(pending?.callId, 'call-1')
  assert.equal(typeof pending.revision, 'number')
  const approval = await coordinator.fetch(
    internal('/approval', actor, { callId: 'call-1', decision: 'allow' }),
  )
  assert.equal(approval.status, 200)
  assert.equal(
    (
      await coordinator.fetch(
        internal('/approval', actor, { callId: 'call-1', decision: 'deny' }),
      )
    ).status,
    409,
  )
  beforeWait.resolve()
  assert.equal(await decision.promise, true)
  await state.drain()
  const stored = JSON.stringify(state.storage.writes)
  assert.ok(!stored.includes('private args'))
  assert.ok(!stored.includes('private prompt'))
  const finished = await state.storage.get<StoredRun>('run')
  assert.ok(finished)
  assert.equal(finished?.pending, null)
  assert.equal(finished.phase, 'completed')
})

test('stream cancellation signals execution and waits for in-flight cleanup before persisting terminal', async () => {
  const signalled = Promise.withResolvers<void>()
  const cleaned = Promise.withResolvers<void>()
  const { coordinator, state, accept } = fixture({
    async execute(_prepared, signal) {
      if (signal.aborted) signalled.resolve()
      else
        signal.addEventListener('abort', () => signalled.resolve(), {
          once: true,
        })
      await signalled.promise
      await cleaned.promise
      return snapshot('cancelled')
    },
  })
  await accept()
  const response = await coordinator.fetch(internal('/events'))
  let settled = false
  const cancellation = response.body?.cancel().then(() => {
    settled = true
  })
  await signalled.promise
  const cancelling = await state.storage.get<StoredRun>('run')
  assert.ok(cancelling)
  assert.equal(cancelling?.phase, 'running')
  assert.equal(cancelling.snapshot.cancelRequested, true)
  assert.equal(settled, false)
  cleaned.resolve()
  await cancellation
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'cancelled')
  await state.drain()
})

test('request abort cancels active execution while the POST request signal no longer owns an accepted run', async () => {
  const post = new AbortController(),
    subscription = new AbortController()
  const stopped = Promise.withResolvers<void>()
  const { coordinator, state, accept } = fixture({
    async execute(_prepared, signal) {
      if (signal.aborted) stopped.resolve()
      else
        signal.addEventListener('abort', () => stopped.resolve(), {
          once: true,
        })
      await stopped.promise
      return snapshot('cancelled')
    },
  })
  await accept(actor, post.signal)
  post.abort()
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'accepted')
  await coordinator.fetch(
    internal('/events', actor, undefined, subscription.signal),
  )
  subscription.abort()
  await stopped.promise
  await state.drain()
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'cancelled')
})

for (const phase of ['creating', 'accepted', 'running'] as const) {
  test(`restart recovers the planned reservation for ${phase} without replay`, async () => {
    const storage = new Storage()
    await storage.put('run', {
      phase,
      actor,
      runId: handle.runId,
      handle,
      snapshot: snapshot('running'),
      revision: 4,
      acceptedUntil: 1,
      pending: { callId: 'old-call', revision: 4 },
    })
    let recoveries = 0,
      starts = 0
    const { coordinator, state } = fixture(
      {
        async recover(planned, context) {
          recoveries++
          assert.deepEqual(planned, handle)
          assert.deepEqual(context.actor, actor)
        },
        async execute() {
          starts++
          return snapshot('completed')
        },
      },
      storage,
    )
    const response = await coordinator.fetch(internal('/snapshot'))
    assert.equal(response.status, 200)
    const recovered: unknown = await response.json()
    assert.ok(isContract('RunSnapshot', recovered))
    assert.equal(recovered.status, 'failed')
    assert.ok(recovered.error?.details)
    assert.equal(recovered.error?.code, 'internal')
    assert.equal(recovered.error.details?.reason, 'interrupted')
    assert.equal(typeof recovered.error.details.errorId, 'string')
    assert.equal(recoveries, 1)
    assert.equal(starts, 0)
    assert.ok(
      (await (await coordinator.fetch(internal('/events'))).text()).includes(
        '"status":"failed"',
      ),
    )
    const record = await storage.get<StoredRun>('run')
    assert.ok(record)
    assert.equal(record?.pending, null)
    assert.equal(record.phase, 'failed')
    await state.drain()
  })
}

test('creating uncommitted planned handle may be absent, but committed accepted recovery errors are not ignored', async () => {
  for (const phase of ['creating', 'accepted'] as const) {
    const storage = new Storage()
    await storage.put('run', {
      phase,
      actor,
      runId: handle.runId,
      handle,
      snapshot: snapshot('running'),
      revision: 1,
      acceptedUntil: null,
      pending: null,
    })
    const { coordinator } = fixture(
      {
        async recover() {
          throw new KitFailure('not_found', 'private missing reservation')
        },
      },
      storage,
    )
    const response = await coordinator.fetch(internal('/snapshot'))
    assert.equal(response.status, phase === 'creating' ? 200 : 404)
    const record = await storage.get<StoredRun>('run')
    assert.ok(record)
    assert.equal(record?.phase, 'failed')
    assert.equal(record.snapshot.status, 'failed')
  }
})

test('runtime End is not published until authoritative snapshot exists and errors never persist host secrets or forged error IDs', async () => {
  const saved = Promise.withResolvers<void>()
  const emitted = Promise.withResolvers<void>()
  const { coordinator, state, accept } = fixture({
    async execute(_prepared, _signal, emit) {
      emit({ kind: 'text_delta', text: 'hello' })
      emit({ kind: 'end', status: 'completed' })
      emitted.resolve()
      await saved.promise
      return {
        ...snapshot('failed'),
        error: {
          code: 'internal',
          message: 'Bearer secret-value private prompt',
          details: { errorId: 'secret-value', reason: 'private args' },
        },
      }
    },
  })
  await accept()
  const response = await coordinator.fetch(internal('/events'))
  const reader = response.body?.getReader()
  assert.ok(reader)
  await emitted.promise
  const first = await reader.read()
  assert.ok(new TextDecoder().decode(first.value).includes('text_delta'))
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'running')
  saved.resolve()
  const end = await reader.read()
  const text = new TextDecoder().decode(end.value)
  assert.ok(text.includes('"kind":"end","status":"failed"'))
  assert.ok(!text.includes('secret-value'))
  assert.ok(!text.includes('private prompt'))
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'failed')
  const stored = JSON.stringify(state.storage.writes)
  assert.ok(!stored.includes('secret-value'))
  assert.ok(!stored.includes('private args'))
  const final: unknown = await (
    await coordinator.fetch(internal('/snapshot'))
  ).json()
  assert.ok(isContract('RunSnapshot', final))
  await state.drain()
})

test('HTTP transport cancellation explicitly cancels the run when RPC body cancellation does not propagate', async (context) => {
  context.mock.timers.enable({ apis: ['setInterval'] })
  const stopped = Promise.withResolvers<void>()
  const { coordinator, state, accept } = fixture({
    async execute(_prepared, signal, emit) {
      emit({
        kind: 'approval_required',
        callId: 'transport-call',
        name: 'write',
        argsPreview: '{}',
      })
      if (signal.aborted) stopped.resolve()
      else
        signal.addEventListener('abort', () => stopped.resolve(), {
          once: true,
        })
      await stopped.promise
      return snapshot('cancelled')
    },
  })
  await accept()
  let explicitCancels = 0,
    rpcCancels = 0
  const app = createAetherWorkerApp({
    applicationId: actor.applicationId,
    origin: 'https://product.example',
    identity: {
      async resolve() {
        return actor
      },
      async verifyFresh() {
        return true
      },
    },
    async authorizeRoute() {},
    async verifyCsrf() {
      return true
    },
    runs: {
      idFromName(name) {
        return name
      },
      get() {
        return {
          async fetch(target, init) {
            const request = new Request(target, init)
            const path = new URL(request.url).pathname
            if (path === '/cancel') {
              explicitCancels++
              assert.equal(request.signal.aborted, false)
              assert.equal(
                request.headers.get('x-aether-actor'),
                internal('/snapshot').headers.get('x-aether-actor'),
              )
            }
            const reply = await coordinator.fetch(request)
            if (path !== '/events' || reply.status !== 200 || !reply.body)
              return reply
            const reader = reply.body.getReader()
            // Model an RPC boundary whose downstream cancel never reaches the DO stream.
            const detached = new ReadableStream<Uint8Array>({
              async pull(controller) {
                const chunk = await reader.read()
                if (chunk.done) controller.close()
                else controller.enqueue(chunk.value)
              },
              cancel() {
                rpcCancels++
              },
            })
            return new Response(detached, { headers: reply.headers })
          },
        }
      },
    },
  })
  const request = new Request(
    `https://product.example/api/v1/runs/${handle.runId}/events`,
  )
  const response = await app.fetch(request)
  const reader = response.body?.getReader()
  assert.ok(reader)
  assert.ok(
    new TextDecoder()
      .decode((await reader.read()).value)
      .includes('approval_required'),
  )
  // Heartbeats are SSE comments, not synthetic RunEvents or replays.
  const heartbeat = reader.read()
  context.mock.timers.tick(1000)
  assert.equal(
    new TextDecoder().decode((await heartbeat).value),
    ': keep-alive\n\n',
  )
  const competitor = await app.fetch(new Request(request))
  assert.equal(competitor.status, 409)
  assert.equal(explicitCancels, 0)
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'running')
  await reader.cancel()
  await stopped.promise
  await state.drain()
  assert.equal(explicitCancels, 1)
  assert.equal(rpcCancels, 1)
  assert.equal((await state.storage.get<StoredRun>('run'))?.phase, 'cancelled')
  assert.equal(request.signal.aborted, false)
})
