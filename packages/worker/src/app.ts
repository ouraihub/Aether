import { isContract } from '@ouraihub/aether-contracts'
import type { KitError, KitErrorCode } from '@ouraihub/aether-contracts'
import { badRequest, KitFailure, safeError } from '@ouraihub/aether-core/error'
import type { DurableObjectNamespace } from './cloudflare.ts'
import { immutableActor, isActor } from './context.ts'
import type { Actor, WorkerContext } from './context.ts'
import type { IdentityPort } from './identity.ts'
import type { Logger } from './logger.ts'

export interface WorkerHandler {
  handle(request: Request, context: WorkerContext): Promise<Response | null>
}
export interface PublicWorkerHandler {
  handlePublic(request: Request): Promise<Response | null>
}
export interface WorkerAppOptions {
  readonly applicationId: string
  readonly origin: string
  readonly identity: IdentityPort
  readonly runs: DurableObjectNamespace
  readonly handlers?: readonly WorkerHandler[]
  readonly publicHandlers?: readonly PublicWorkerHandler[]
  /** Authentication does not imply resource authorization or admission quota. */
  readonly authorizeRoute: (actor: Actor, request: Request) => Promise<void>
  readonly verifyCsrf: (request: Request, actor: Actor) => Promise<boolean>
  readonly logger?: Logger
  readonly maxBodyBytes?: number
  readonly allowLoopbackHttp?: boolean
}
const statuses: Record<KitErrorCode, number> = {
  bad_request: 400,
  not_found: 404,
  conflict: 409,
  unsupported: 422,
  cancelled: 409,
  unauthorized: 401,
  forbidden: 403,
  rate_limited: 429,
  upstream: 502,
  timeout: 504,
  network: 502,
  needs_config: 422,
  budget_exceeded: 413,
  incompatible_db: 503,
  internal: 500,
}
function failureResponse(error: unknown): Response {
  let safe: KitError = safeError(error)
  if (safe.code === 'internal')
    safe = {
      code: 'internal',
      message: 'Internal request failure',
      details: { errorId: crypto.randomUUID() },
    }
  return Response.json(safe, { status: statuses[safe.code] })
}
function loopback(url: URL): boolean {
  return (
    url.hostname === 'localhost' ||
    url.hostname === '127.0.0.1' ||
    url.hostname === '[::1]'
  )
}
async function boundedRequest(
  request: Request,
  maximum: number,
): Promise<Request> {
  if (
    request.method === 'GET' ||
    request.method === 'HEAD' ||
    request.body === null
  )
    return request
  const declared = request.headers.get('content-length')
  if (
    declared !== null &&
    (!/^\d+$/u.test(declared) || Number(declared) > maximum)
  )
    throw badRequest('body', 'too_large', maximum)
  const reader = request.body.getReader(),
    chunks: Uint8Array[] = []
  let size = 0,
    completed = false
  try {
    while (true) {
      const next = await reader.read()
      if (next.done) {
        completed = true
        break
      }
      size += next.value.byteLength
      if (size > maximum) throw badRequest('body', 'too_large', maximum)
      chunks.push(next.value)
    }
  } finally {
    if (!completed) await reader.cancel().catch(() => undefined)
    reader.releaseLock()
  }
  const bytes = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return new Request(request, { body: bytes })
}
function actorHeader(actor: Actor): string {
  const bytes = new TextEncoder().encode(JSON.stringify(actor))
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}
function secured(
  response: Response,
  origin: string | null,
  allowed: string,
): Response {
  const headers = new Headers(response.headers)
  headers.set('X-Content-Type-Options', 'nosniff')
  headers.set('Referrer-Policy', 'no-referrer')
  headers.set(
    'Cache-Control',
    headers.get('content-type')?.startsWith('text/event-stream')
      ? 'no-cache, no-transform'
      : 'no-store',
  )
  if (origin === allowed) {
    headers.set('Access-Control-Allow-Origin', allowed)
    headers.set('Access-Control-Allow-Credentials', 'true')
    headers.append('Vary', 'Origin')
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}
/** Product entrypoints compose this handler; this package never supplies product login or business tools. */
export function createAetherWorkerApp(options: WorkerAppOptions): {
  fetch(request: Request): Promise<Response>
} {
  const publicOrigin = new URL(options.origin)
  if (
    !options.applicationId ||
    publicOrigin.origin !== options.origin ||
    (publicOrigin.protocol !== 'https:' &&
      !(
        options.allowLoopbackHttp &&
        publicOrigin.protocol === 'http:' &&
        loopback(publicOrigin)
      ))
  )
    throw new KitFailure('needs_config', 'Invalid Worker origin or application')
  const maximum = options.maxBodyBytes ?? 24 * 1024 * 1024
  if (
    !Number.isSafeInteger(maximum) ||
    maximum <= 0 ||
    maximum > 32 * 1024 * 1024
  )
    throw badRequest('maxBodyBytes', 'invalid')
  return {
    async fetch(raw: Request): Promise<Response> {
      const now = Date.now(),
        requestId = crypto.randomUUID(),
        url = new URL(raw.url),
        origin = raw.headers.get('origin')
      let response: Response
      try {
        if (
          url.protocol !== 'https:' &&
          !(
            options.allowLoopbackHttp &&
            url.protocol === 'http:' &&
            loopback(url)
          )
        )
          throw new KitFailure('forbidden', 'Secure transport required')
        if (origin !== null && origin !== options.origin)
          throw new KitFailure('forbidden', 'Origin rejected')
        if (raw.method === 'OPTIONS') {
          if (origin !== options.origin)
            throw new KitFailure('forbidden', 'Origin rejected')
          return secured(
            new Response(null, {
              status: 204,
              headers: {
                'Access-Control-Allow-Methods':
                  'GET, POST, PUT, DELETE, OPTIONS',
                'Access-Control-Allow-Headers': 'Content-Type, X-CSRF-Token',
                'Access-Control-Max-Age': '600',
              },
            }),
            origin,
            options.origin,
          )
        }
        const mutating = !['GET', 'HEAD'].includes(raw.method)
        const publicRoute =
          url.pathname === '/mcp' ||
          url.pathname.startsWith('/oauth/') ||
          url.pathname.startsWith('/.well-known/')
        if (
          mutating &&
          raw.body !== null &&
          !publicRoute &&
          raw.headers
            .get('content-type')
            ?.split(';')[0]
            ?.trim()
            .toLowerCase() !== 'application/json'
        )
          throw badRequest('contentType', 'json_required')
        const request = await boundedRequest(raw, maximum)
        if (publicRoute) {
          let publicResponse: Response | null = null
          for (const handler of options.publicHandlers ?? []) {
            publicResponse = await handler.handlePublic(request)
            if (publicResponse) break
          }
          if (!publicResponse)
            throw new KitFailure('not_found', 'Unknown route', {
              entity: 'route',
            })
          response = publicResponse
        } else {
          const actor = await options.identity.resolve(request)
          if (!isActor(actor) || actor.applicationId !== options.applicationId)
            throw new KitFailure('unauthorized', 'Authentication required')
          if (
            mutating &&
            actor.kind === 'user' &&
            actor.sessionId !== null &&
            (origin !== options.origin ||
              !(await options.verifyCsrf(request, actor)))
          )
            throw new KitFailure('forbidden', 'Request authorization rejected')
          await options.authorizeRoute(actor, request)
          const context: WorkerContext = {
            actor: immutableActor(actor),
            requestId,
            now,
            signal: request.signal,
          }
          if (url.pathname === '/api/v1/runs' && request.method === 'POST') {
            const input: unknown = await request.json()
            if (!isContract('RunRequest', input))
              throw badRequest('run', 'invalid')
            const runId = crypto.randomUUID(),
              stub = options.runs.get(
                options.runs.idFromName(`${options.applicationId}:${runId}`),
              )
            const accepted = await stub.fetch(
              'https://aether.internal/accept',
              {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                  actor: context.actor,
                  request: input,
                  runId,
                }),
                signal: request.signal,
              },
            )
            const value: unknown = await accepted.json()
            if (accepted.status !== 202) {
              if (isContract('KitError', value))
                throw new KitFailure(value.code, value.message, value.details)
              throw new KitFailure('internal', 'Invalid coordinator reply')
            }
            if (!isContract('RunAccepted', value))
              throw new KitFailure('internal', 'Invalid coordinator acceptance')
            response = Response.json(
              {
                accepted: value,
                events: `/api/v1/runs/${encodeURIComponent(runId)}/events`,
              },
              { status: 202 },
            )
          } else {
            const run =
              /^\/api\/v1\/runs\/([^/]+)(?:\/(events|cancel|approval))?$/u.exec(
                url.pathname,
              )
            if (run) {
              const runId = decodeURIComponent(run[1] ?? ''),
                action = run[2] ?? 'snapshot'
              if (
                !/^[A-Za-z0-9_-]{1,128}$/u.test(runId) ||
                (['events', 'snapshot'].includes(action)
                  ? request.method !== 'GET'
                  : request.method !== 'POST')
              )
                throw badRequest('run', 'invalid')
              const stub = options.runs.get(
                  options.runs.idFromName(`${options.applicationId}:${runId}`),
                ),
                headers = new Headers({
                  'x-aether-actor': actorHeader(context.actor),
                })
              if (request.headers.has('Last-Event-ID'))
                headers.set(
                  'Last-Event-ID',
                  request.headers.get('Last-Event-ID') ?? '',
                )
              const body =
                request.method === 'POST' ? await request.text() : undefined
              if (body !== undefined)
                headers.set('Content-Type', 'application/json')
              response = await stub.fetch(`https://aether.internal/${action}`, {
                method: request.method,
                headers,
                ...(body === undefined ? {} : { body }),
                signal: request.signal,
              })
              if (
                action === 'events' &&
                response.status === 200 &&
                response.body &&
                response.headers
                  .get('content-type')
                  ?.startsWith('text/event-stream')
              ) {
                const upstream = response.body.getReader()
                const transport = new TransformStream<Uint8Array, Uint8Array>()
                const writer = transport.writable.getWriter()
                const heartbeat = new TextEncoder().encode(': keep-alive\n\n')
                let finished = false,
                  heartbeatPending = false
                let cancellation: Promise<void> | null = null
                const disconnected = (): Promise<void> => {
                  if (finished) return Promise.resolve()
                  if (cancellation) return cancellation
                  clearInterval(timer)
                  request.signal.removeEventListener('abort', aborted)
                  // RPC response-body cancellation is not a reliable cross-isolate signal.
                  // Explicitly notify the same authenticated run without the aborted client signal.
                  cancellation = Promise.all([
                    stub
                      .fetch('https://aether.internal/cancel', {
                        method: 'POST',
                        headers: {
                          'x-aether-actor': actorHeader(context.actor),
                          'Content-Type': 'application/json',
                        },
                        body: '{}',
                      })
                      .then(async (reply) => {
                        await reply.body?.cancel()
                      }),
                    upstream.cancel(),
                    writer.abort(),
                  ]).then(() => undefined)
                  return cancellation
                }
                const aborted = (): void => {
                  void disconnected().catch(() => undefined)
                }
                // A comment forces a real transport write even while the engine awaits approval.
                const timer = setInterval(() => {
                  if (finished || cancellation || heartbeatPending) return
                  heartbeatPending = true
                  void writer
                    .write(heartbeat)
                    .catch(disconnected)
                    .finally(() => {
                      heartbeatPending = false
                    })
                    .catch(() => undefined)
                }, 1000)
                request.signal.addEventListener('abort', aborted, {
                  once: true,
                })
                void writer.closed.catch(disconnected).catch(() => undefined)
                const pump = async (): Promise<void> => {
                  try {
                    while (!cancellation) {
                      const next = await upstream.read()
                      if (next.done) {
                        clearInterval(timer)
                        await writer.close()
                        finished = true
                        break
                      }
                      await writer.write(next.value)
                    }
                  } catch {
                    await disconnected()
                  } finally {
                    clearInterval(timer)
                    request.signal.removeEventListener('abort', aborted)
                    upstream.releaseLock()
                    writer.releaseLock()
                  }
                }
                void pump().catch(() => undefined)
                if (request.signal.aborted) aborted()
                response = new Response(transport.readable, {
                  status: response.status,
                  statusText: response.statusText,
                  headers: response.headers,
                })
              }
            } else {
              let handled: Response | null = null
              for (const handler of options.handlers ?? []) {
                handled = await handler.handle(request, context)
                if (handled) break
              }
              if (!handled)
                throw new KitFailure('not_found', 'Unknown route', {
                  entity: 'route',
                })
              response = handled
            }
          }
        }
      } catch (error) {
        response = failureResponse(error)
      }
      options.logger?.log('request.finished', {
        requestId,
        applicationId: options.applicationId,
        status: response.status,
        durationMs: Date.now() - now,
      })
      return secured(response, origin, options.origin)
    },
  }
}
