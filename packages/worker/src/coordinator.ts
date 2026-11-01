import { isContract, type RunEvent, type RunSnapshot } from '@ouraihub/aether-contracts'
import { KitFailure, type Approver } from '@ouraihub/aether-core'
import type { DurableObjectState } from './cloudflare.js'
import { immutableActor, isActor, ownerKey, type Actor, type WorkerContext } from './context.js'
import type { PreparedExecution, WorkerRuntime } from './runtime.js'
import {
  actorHeader,
  conflict,
  encoder,
  endEvent,
  interrupted,
  missing,
  publicError,
  statusCode,
  storageKey,
  terminal,
  type Metadata,
  type Pending,
} from './coordinator-helpers.js'

type Runtime = Pick<WorkerRuntime, 'prepare' | 'execute' | 'abandon' | 'recover'>
/** Time and identifiers the coordinator uses for leases, request ids and error ids. */
export interface CoordinatorOptions {
  /** Milliseconds since the epoch; drives the 30 s subscription lease and alarms. */
  readonly now?: () => number
  readonly newId?: () => string
}

/** The router alone constructs this private protocol after authentication. */
export function createRunCoordinator<Env>(
  createRuntime: (env: Env) => Runtime,
  options: CoordinatorOptions = {},
) {
  const now = options.now ?? Date.now
  const newId = options.newId ?? (() => crypto.randomUUID())
  return class RunCoordinator {
    private readonly state: DurableObjectState
    private readonly runtime: Runtime
    private readonly ready: Promise<void>
    private queue: Promise<void> = Promise.resolve()
    private metadata: Metadata | null = null
    private prepared: PreparedExecution | null = null
    private readonly abort = new AbortController()
    private operation: Promise<void> | null = null
    private pending: Pending | null = null
    private controller: ReadableStreamDefaultController<Uint8Array> | null = null
    private detach: (() => void) | null = null
    private sequence = 0
    private subscribed = false

    constructor(state: DurableObjectState, env: Env) {
      this.state = state
      this.runtime = createRuntime(env)
      this.ready = state.blockConcurrencyWhile(async () => {
        this.metadata = (await state.storage.get<Metadata>(storageKey)) ?? null
        const record = this.metadata
        if (!record || terminal(record.phase)) return
        let recoveryFailure: unknown
        if (record.handle) {
          try {
            await this.runtime.recover(record.handle, this.context(record.actor))
          } catch (error) {
            if (!(
              record.phase === 'creating' &&
              error instanceof KitFailure &&
              error.code === 'not_found'
            ))
              recoveryFailure = error
          }
        }
        record.phase = 'failed'
        record.snapshot = { ...record.snapshot, status: 'failed', error: interrupted(newId) }
        record.pending = null
        record.acceptedUntil = null
        await this.persist()
        await state.storage.deleteAlarm()
        if (recoveryFailure !== undefined) throw recoveryFailure
      })
    }

    private context(actor: Actor): WorkerContext {
      return { actor, signal: this.abort.signal, requestId: newId(), now: now() }
    }
    private serialize<T>(action: () => Promise<T>): Promise<T> {
      const result = this.queue.then(action)
      this.queue = result.then(
        () => undefined,
        () => undefined,
      )
      return result
    }
    private async persist(): Promise<void> {
      if (!this.metadata) return
      this.metadata.revision++
      await this.state.storage.put(storageKey, this.metadata)
    }
    private owns(actor: Actor): boolean {
      return (
        this.metadata !== null &&
        ownerKey(actor) === ownerKey(this.metadata.actor) &&
        actor.sessionId === this.metadata.actor.sessionId
      )
    }
    private publish(event: RunEvent): void {
      const controller = this.controller
      if (!controller) return
      try {
        // Bound an unread subscriber rather than retaining an unbounded transcript in memory.
        if (controller.desiredSize !== null && controller.desiredSize <= 0)
          throw new Error('Subscriber stalled')
        controller.enqueue(
          encoder.encode(
            `event: run_event\nid: ${++this.sequence}\ndata: ${JSON.stringify(event)}\n\n`,
          ),
        )
      } catch {
        this.close()
        this.state.waitUntil(this.cancelRun())
      }
    }
    private close(): void {
      const controller = this.controller
      this.controller = null
      this.detach?.()
      this.detach = null
      try {
        controller?.close()
      } catch {
        /* The consumer may already have cancelled. */
      }
    }
    private receive = (event: RunEvent): void => {
      if (event.kind === 'end' || this.abort.signal.aborted) return
      let approval: Pending | null = null
      if (event.kind === 'approval_required') {
        if (this.pending) throw conflict('approval_already_pending')
        approval = {
          callId: event.callId,
          revision: (this.metadata?.revision ?? 0) + 1,
          decision: undefined,
          resolve: null,
        }
        this.pending = approval
      }
      const registered = approval
      const write = this.serialize(async () => {
        if (!this.metadata || terminal(this.metadata.phase) || this.abort.signal.aborted) return
        if (registered) {
          this.metadata.pending = { callId: registered.callId, revision: registered.revision }
          await this.persist()
        }
        this.publish(event)
      })
      this.state.waitUntil(write.catch(() => this.cancelRun()))
    }
    private readonly approver: Approver = {
      wait: async (runId, call, signal) => {
        const pending = this.pending
        if (!pending || pending.callId !== call.id || this.metadata?.runId !== runId)
          throw conflict('approval_not_pending')
        if (signal.aborted || this.abort.signal.aborted) return false
        let decision = pending.decision
        if (decision === undefined) {
          const { promise, resolve } = Promise.withResolvers<boolean>()
          const abort = (): void => resolve(false)
          pending.resolve = resolve
          signal.addEventListener('abort', abort, { once: true })
          this.abort.signal.addEventListener('abort', abort, { once: true })
          if (signal.aborted || this.abort.signal.aborted) resolve(false)
          try {
            decision = await promise
          } finally {
            signal.removeEventListener('abort', abort)
            this.abort.signal.removeEventListener('abort', abort)
            pending.resolve = null
          }
        }
        await this.serialize(async () => {
          if (this.pending !== pending || !this.metadata) return
          this.pending = null
          this.metadata.pending = null
          await this.persist()
        })
        return decision
      },
    }
    private async finish(snapshot: RunSnapshot): Promise<void> {
      await this.serialize(async () => {
        const record = this.metadata
        if (!record || terminal(record.phase)) return
        if (snapshot.status === 'running') throw conflict('runtime_not_terminal')
        // Rebuild the DTO: no runtime-private additions or unsanitized error details persist.
        record.snapshot = {
          runId: record.runId,
          conversationId: record.snapshot.conversationId,
          turnId: record.snapshot.turnId,
          status: snapshot.status,
          cancelRequested: record.snapshot.cancelRequested || snapshot.cancelRequested,
          ...(snapshot.error ? { error: publicError(snapshot.error, newId) } : {}),
          ...(snapshot.exhausted === undefined ? {} : { exhausted: snapshot.exhausted }),
        }
        record.phase = snapshot.status
        record.pending = null
        record.acceptedUntil = null
        this.pending?.resolve?.(false)
        this.pending = null
        await this.persist()
        await this.state.storage.deleteAlarm()
        this.publish(endEvent(record.snapshot))
        this.close()
        this.prepared = null
      })
    }
    private async fail(error: unknown): Promise<void> {
      const record = this.metadata
      if (!record || terminal(record.phase)) return
      if (record.handle) {
        try {
          await this.runtime.recover(record.handle, this.context(record.actor))
        } catch (recoveryError) {
          if (!(
            record.phase === 'creating' &&
            recoveryError instanceof KitFailure &&
            recoveryError.code === 'not_found'
          ))
            throw recoveryError
        }
      }
      await this.finish({ ...record.snapshot, status: 'failed', error: publicError(error, newId) })
    }
    private async run(prepared: PreparedExecution, execute: boolean): Promise<void> {
      try {
        const snapshot = execute
          ? await this.runtime.execute(prepared, this.abort.signal, this.receive, this.approver)
          : await this.runtime.abandon(prepared, 'cancelled')
        await this.finish(snapshot)
      } catch (error) {
        await this.fail(error)
      }
    }
    private async cancelRun(): Promise<void> {
      await this.serialize(async () => {
        const record = this.metadata
        if (!record || terminal(record.phase)) return
        this.abort.abort()
        this.pending?.resolve?.(false)
        record.snapshot.cancelRequested = true
        await this.persist()
        if ((record.phase === 'accepted' || !this.operation) && this.prepared) {
          const prepared = this.prepared
          record.phase = 'running'
          record.acceptedUntil = null
          await this.persist()
          this.operation = Promise.resolve().then(() => this.run(prepared, false))
          this.state.waitUntil(this.operation)
        }
      })
      await this.operation
    }
    private async accept(request: Request): Promise<Response> {
      let value: unknown
      try {
        value = await request.json()
      } catch {
        throw new KitFailure('bad_request', 'Invalid JSON')
      }
      if (
        typeof value !== 'object' ||
        value === null ||
        !('actor' in value) ||
        !isActor(value.actor) ||
        !('runId' in value) ||
        typeof value.runId !== 'string' ||
        !/^[A-Za-z0-9_-]{1,128}$/u.test(value.runId) ||
        !('request' in value) ||
        !isContract('RunRequest', value.request)
      )
        throw new KitFailure('bad_request', 'Invalid run request')
      const actor = immutableActor(value.actor),
        runId = value.runId,
        input = value.request
      await this.serialize(async () => {
        if (this.metadata) throw conflict('run_already_exists')
        this.metadata = {
          phase: 'creating',
          actor,
          runId,
          handle: null,
          revision: 0,
          pending: null,
          acceptedUntil: null,
          snapshot: {
            runId,
            conversationId: null,
            turnId: null,
            status: 'running',
            cancelRequested: false,
          },
        }
        await this.persist()
      })
      const abort = (): void => {
        this.state.waitUntil(this.cancelRun())
      }
      request.signal.addEventListener('abort', abort, { once: true })
      if (request.signal.aborted) this.abort.abort()
      let accepted = false
      const preparation = async (): Promise<void> => {
        try {
          const prepared = await this.runtime.prepare(runId, input, this.context(actor), (handle) =>
            this.serialize(async () => {
              if (!this.metadata || this.abort.signal.aborted)
                throw new KitFailure('cancelled', 'Cancelled')
              this.metadata.handle = handle
              this.metadata.snapshot.conversationId = handle?.conversationId ?? null
              this.metadata.snapshot.turnId = handle?.turnId ?? null
              await this.persist()
            }),
          )
          this.prepared = prepared
          if (this.abort.signal.aborted) {
            await this.run(prepared, false)
            return
          }
          await this.serialize(async () => {
            if (!this.metadata) throw missing()
            if (this.abort.signal.aborted) throw new KitFailure('cancelled', 'Cancelled')
            this.metadata.phase = 'accepted'
            this.metadata.snapshot.conversationId = prepared.accepted.conversationId
            this.metadata.snapshot.turnId = prepared.accepted.turnId
            this.metadata.acceptedUntil = now() + 30000
            await this.state.storage.setAlarm(this.metadata.acceptedUntil)
            await this.persist()
          })
          accepted = true
        } catch (error) {
          if (this.prepared) await this.run(this.prepared, false)
          else await this.fail(error)
          throw error
        } finally {
          request.signal.removeEventListener('abort', abort)
        }
      }
      const work = preparation()
      this.operation = work
      this.state.waitUntil(work.catch(() => undefined))
      await work
      if (!accepted || !this.prepared) throw new KitFailure('cancelled', 'Cancelled')
      return Response.json(this.prepared.accepted, { status: 202 })
    }
    private async events(request: Request, actor: Actor): Promise<Response> {
      try {
        return await this.serialize(async () => {
          const record = this.metadata
          if (!record || !this.owns(actor)) throw missing()
          if (request.headers.has('Last-Event-ID')) throw conflict('event_replay_unavailable')
          if (request.signal.aborted) throw new KitFailure('cancelled', 'Cancelled')
          if (!terminal(record.phase) && (record.phase !== 'accepted' || this.subscribed))
            throw conflict('subscriber_already_active')
          const stream = new ReadableStream<Uint8Array>(
            {
              start: (controller) => {
                this.controller = controller
              },
              cancel: async () => {
                this.close()
                await this.cancelRun()
              },
            },
            { highWaterMark: 1024 * 1024, size: (chunk) => chunk.byteLength },
          )
          if (terminal(record.phase)) {
            this.publish(endEvent(record.snapshot))
            this.close()
          } else {
            if (!this.prepared) throw conflict('prepared_execution_unavailable')
            this.subscribed = true
            record.phase = 'running'
            this.operation = null
            record.acceptedUntil = null
            await this.persist()
            await this.state.storage.deleteAlarm()
            const abort = (): void => {
              this.close()
              this.state.waitUntil(this.cancelRun())
            }
            request.signal.addEventListener('abort', abort, { once: true })
            this.detach = () => request.signal.removeEventListener('abort', abort)
            const prepared = this.prepared
            this.operation = Promise.resolve().then(() => this.run(prepared, true))
            this.state.waitUntil(this.operation)
            if (request.signal.aborted) abort()
          }
          return new Response(stream, {
            headers: {
              'Content-Type': 'text/event-stream',
              'Cache-Control': 'no-cache, no-transform',
              'X-Accel-Buffering': 'no',
              'X-Content-Type-Options': 'nosniff',
            },
          })
        })
      } catch (error) {
        // A rejected competing subscriber must never cancel the actual owner stream.
        if (
          this.owns(actor) &&
          ((this.subscribed && !this.operation) ||
            (this.metadata?.phase === 'accepted' && request.signal.aborted))
        ) {
          this.close()
          await this.cancelRun()
        }
        throw error
      }
    }
    async fetch(request: Request): Promise<Response> {
      try {
        await this.ready
        const path = new URL(request.url).pathname
        if (request.method === 'POST' && path === '/accept') return await this.accept(request)
        const actor = actorHeader(request)
        if (request.method === 'POST' && path === '/cancel') {
          if (this.owns(actor)) await this.cancelRun()
          return Response.json({ ok: true })
        }
        if (!this.owns(actor)) throw missing()
        if (request.method === 'GET' && path === '/events') return await this.events(request, actor)
        if (request.method === 'GET' && path === '/snapshot')
          return await this.serialize(async () => Response.json(this.metadata?.snapshot))
        if (request.method === 'POST' && path === '/approval') {
          let value: unknown
          try {
            value = await request.json()
          } catch {
            throw new KitFailure('bad_request', 'Invalid JSON')
          }
          if (
            typeof value !== 'object' ||
            value === null ||
            !('callId' in value) ||
            typeof value.callId !== 'string' ||
            !('decision' in value) ||
            (value.decision !== 'allow' && value.decision !== 'deny')
          )
            throw new KitFailure('bad_request', 'Invalid approval')
          const callId = value.callId,
            allowed = value.decision === 'allow'
          return await this.serialize(async () => {
            const pending = this.pending,
              stored = this.metadata?.pending
            if (
              !pending ||
              !stored ||
              pending.callId !== callId ||
              stored.callId !== pending.callId ||
              stored.revision !== pending.revision ||
              pending.decision !== undefined ||
              this.abort.signal.aborted ||
              this.metadata?.phase !== 'running'
            )
              throw conflict('approval_not_pending')
            pending.decision = allowed
            this.metadata.pending = null
            await this.persist()
            pending.resolve?.(pending.decision)
            return Response.json({ ok: true })
          })
        }
        throw missing()
      } catch (error) {
        const safe = publicError(error, newId)
        return Response.json(safe, { status: statusCode(safe) })
      }
    }
    async alarm(): Promise<void> {
      await this.ready
      const record = this.metadata
      if (record?.phase === 'accepted' && record.acceptedUntil !== null) {
        if (now() >= record.acceptedUntil) await this.cancelRun()
        else
          await this.serialize(async () => {
            if (this.metadata?.phase === 'accepted' && this.metadata.acceptedUntil !== null)
              await this.state.storage.setAlarm(this.metadata.acceptedUntil)
          })
      }
    }
  }
}
