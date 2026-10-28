import { badRequest, KitFailure } from '@ouraihub/aether-core'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import type { FetchBinding } from './cloudflare.ts'

export interface OutboundLimits {
  readonly firstByteMs: number
  readonly idleMs: number
  readonly totalMs: number
  readonly maxResponseBytes: number
}
/** This binding MUST enforce public-only DNS at connect time, including all resolved
 * IPv4/IPv6 addresses and rebinding. A DNS preflight followed by ordinary fetch is
 * not this contract. It must not follow redirects or forward through an open proxy.
 * Deploy a trusted egress service binding; absent that binding remote egress fails.
 */
export interface TrustedOutboundGateway {
  readonly security: 'public-addresses-at-connect-time'
  readonly binding: FetchBinding
}
/** Separately deployed, target-bound internal gateway. It must enforce exact
 * configured origin/port at connect time, including DNS, with no redirect/proxy. */
export interface TrustedPrivateOutboundGateway {
  readonly security: 'explicit-private-targets-at-connect-time'
  readonly binding: FetchBinding
  readonly allowedOrigins: readonly string[]
}
export interface WorkerOutboundOptions {
  readonly gateway?: TrustedOutboundGateway
  readonly privateGateway?: TrustedPrivateOutboundGateway
  readonly allowedOrigins: readonly string[]
  readonly limits: OutboundLimits
  /** Local integration tests only, literal loopback hosts; never hostname DNS. */
  readonly allowLoopbackForTests?: boolean
  readonly localFetch?: (url: string, init: RequestInit) => Promise<Response>
}
function isLoopback(hostname: string): boolean {
  return hostname === '[::1]' || /^127(?:\.(?:\d{1,3})){3}$/u.test(hostname)
}
function privateLiteral(host: string): boolean {
  if (host.startsWith('[')) {
    const ip = host.slice(1, -1).toLowerCase()
    // Public IPv6 unicast only; mapped/compatible IPv4, special and local ranges fail closed.
    return (
      !/^[23][0-9a-f]{0,3}:/u.test(ip) ||
      ip.startsWith('2001:db8:') ||
      ip.startsWith('2001:0:') ||
      ip.startsWith('2002:')
    )
  }
  if (!/^\d+(?:\.\d+){3}$/u.test(host))
    return host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local') || !host.includes('.')
  const parts = host.split('.').map(Number)
  const a = parts[0],
    b = parts[1],
    c = parts[2]
  return (
    a === undefined ||
    b === undefined ||
    c === undefined ||
    a === 0 ||
    a === 10 ||
    a === 127 ||
    a >= 224 ||
    (a === 100 && b >= 64 && b <= 127) ||
    (a === 169 && b === 254) ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && (b === 168 || b === 0 || b === 2)) ||
    (a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100))) ||
    (a === 203 && b === 0 && c === 113)
  )
}
export function createWorkerOutbound(options: WorkerOutboundOptions): OutboundHttp {
  const limits = Object.freeze({ ...options.limits })
  for (const value of Object.values(limits))
    if (!Number.isSafeInteger(value) || value <= 0) throw badRequest('outboundLimits', 'invalid')
  const origins = new Set(
    options.allowedOrigins.map((origin) => {
      let url: URL
      try {
        url = new URL(origin)
      } catch {
        throw badRequest('allowedOrigins', 'invalid')
      }
      if (url.origin !== origin || url.username || url.password) throw badRequest('allowedOrigins', 'invalid')
      return origin
    }),
  )
  if (origins.size === 0) throw new KitFailure('needs_config', 'Outbound origins must be explicitly configured')
  const privateOrigins = new Set(options.privateGateway?.allowedOrigins ?? [])
  for (const origin of privateOrigins) {
    let target: URL
    try {
      target = new URL(origin)
    } catch {
      throw badRequest('privateOrigins', 'invalid')
    }
    if (
      target.origin !== origin ||
      !origins.has(origin) ||
      (target.protocol !== 'http:' && target.protocol !== 'https:')
    )
      throw badRequest('privateOrigins', 'invalid')
  }
  return Object.freeze({
    async fetch(url: string, init: RequestInit): Promise<Response> {
      let target: URL
      try {
        target = new URL(url)
      } catch {
        throw badRequest('url', 'invalid')
      }
      const local = options.allowLoopbackForTests === true && isLoopback(target.hostname)
      const privateTarget = privateOrigins.has(target.origin)
      if (
        target.username ||
        target.password ||
        target.hash ||
        (!local && !privateTarget && target.protocol !== 'https:') ||
        ((local || privateTarget) && target.protocol !== 'https:' && target.protocol !== 'http:') ||
        (!local && !privateTarget && privateLiteral(target.hostname)) ||
        !origins.has(target.origin)
      )
        throw badRequest('url', 'outbound_policy')
      const gateway = privateTarget ? options.privateGateway : options.gateway
      if (
        !local &&
        (!gateway ||
          gateway.security !==
            (privateTarget ? 'explicit-private-targets-at-connect-time' : 'public-addresses-at-connect-time'))
      )
        throw new KitFailure('needs_config', 'Trusted connect-time outbound gateway is required')
      const abort = new AbortController()
      let failure: KitFailure | null = null
      const firstTimer = setTimeout(
        () => fail(new KitFailure('timeout', 'Outbound first-byte deadline exceeded')),
        limits.firstByteMs,
      )
      let idleTimer: ReturnType<typeof setTimeout> | undefined
      const totalTimer = setTimeout(
        () => fail(new KitFailure('timeout', 'Outbound total deadline exceeded')),
        limits.totalMs,
      )
      let reader: ReadableStreamDefaultReader<Uint8Array> | undefined
      let bodyController: ReadableStreamDefaultController<Uint8Array> | undefined
      let finished = false
      function clean(): void {
        clearTimeout(firstTimer)
        clearTimeout(idleTimer)
        clearTimeout(totalTimer)
        init.signal?.removeEventListener('abort', callerAbort)
        abort.signal.removeEventListener('abort', onAbort)
      }
      function fail(error: KitFailure): void {
        if (!abort.signal.aborted) {
          failure = error
          abort.abort()
        }
      }
      function callerAbort(): void {
        fail(new KitFailure('cancelled', 'Outbound request cancelled'))
      }
      function onAbort(): void {
        if (finished) return
        finished = true
        bodyController?.error(failure ?? new KitFailure('cancelled', 'Outbound request cancelled'))
        if (reader) void reader.cancel().catch(() => {})
        clean()
      }
      init.signal?.addEventListener('abort', callerAbort, { once: true })
      abort.signal.addEventListener('abort', onAbort, { once: true })
      if (init.signal?.aborted) callerAbort()
      if (abort.signal.aborted) throw failure ?? new KitFailure('cancelled', 'Outbound request cancelled')
      try {
        const headers = new Headers(init.headers)
        headers.set('User-Agent', 'Aether-Worker/1')
        const request: RequestInit = { ...init, headers, redirect: 'manual', signal: abort.signal, credentials: 'omit' }
        // Remote fetch is always the trusted binding. Global fetch is only the literal-loopback opt-in.
        const pending = local
          ? (options.localFetch ?? fetch)(target.href, request)
          : gateway?.binding.fetch(target.href, request)
        if (!pending) throw new KitFailure('needs_config', 'Trusted connect-time outbound gateway is required')
        void pending.then(
          (response) => {
            if (abort.signal.aborted) void response.body?.cancel().catch(() => {})
          },
          () => {},
        )
        const aborted = Promise.withResolvers<never>()
        const rejectAbort = () => aborted.reject(failure ?? new KitFailure('cancelled', 'Outbound request cancelled'))
        abort.signal.addEventListener('abort', rejectAbort, { once: true })
        let response: Response
        try {
          response = await Promise.race([pending, aborted.promise])
        } finally {
          abort.signal.removeEventListener('abort', rejectAbort)
        }
        if (abort.signal.aborted) {
          void response.body?.cancel().catch(() => {})
          throw failure
        }
        if (response.redirected || (response.url && response.url !== target.href)) {
          void response.body?.cancel().catch(() => {})
          throw badRequest('url', 'redirect_forbidden')
        }
        const length = response.headers.get('content-length')
        if (length !== null && (!/^\d+$/u.test(length) || Number(length) > limits.maxResponseBytes)) {
          void response.body?.cancel().catch(() => {})
          throw badRequest('response', 'too_large', limits.maxResponseBytes)
        }
        if (!response.body) {
          finished = true
          clean()
          return response
        }
        reader = response.body.getReader()
        let received = 0
        const body = new ReadableStream<Uint8Array>({
          start(controller) {
            bodyController = controller
          },
          async pull(controller) {
            if (!reader || finished) return
            idleTimer = setTimeout(
              () => fail(new KitFailure('timeout', 'Outbound idle deadline exceeded')),
              limits.idleMs,
            )
            try {
              const result = await reader.read()
              clearTimeout(idleTimer)
              if (finished) return
              if (result.done) {
                finished = true
                clean()
                controller.close()
                reader.releaseLock()
                return
              }
              clearTimeout(firstTimer)
              received += result.value.byteLength
              if (received > limits.maxResponseBytes) {
                fail(badRequest('response', 'too_large', limits.maxResponseBytes))
                return
              }
              controller.enqueue(result.value)
            } catch {
              fail(new KitFailure('internal', 'Outbound response failed'))
            }
          },
          async cancel() {
            finished = true
            clean()
            abort.abort()
            await reader?.cancel().catch(() => {})
          },
        })
        return new Response(body, {
          status: response.status,
          statusText: response.statusText,
          headers: response.headers,
        })
      } catch (error) {
        finished = true
        clean()
        abort.abort()
        if (error instanceof KitFailure) throw error
        throw failure ?? new KitFailure('internal', 'Outbound request failed')
      }
    },
  })
}

export interface TrustedTargetChecker {
  readonly security: 'all-dns-addresses-preflight'
  /** Reject if any A/AAAA result is private/special/unknown, or lookup fails.
   * The content proxy must independently enforce its own connect-time address policy. */
  checkTarget(url: URL, signal: AbortSignal): Promise<void>
}
export function createWorkerTargetCheck(options: {
  readonly checker: TrustedTargetChecker
  readonly allowLoopbackForTests?: boolean
}): (url: URL, signal: AbortSignal) => Promise<void> {
  if (!options.checker || options.checker.security !== 'all-dns-addresses-preflight')
    throw new KitFailure('needs_config', 'Trusted DNS target checker is required')
  return async (url: URL, signal: AbortSignal): Promise<void> => {
    if (signal.aborted) throw new KitFailure('cancelled', 'Target check cancelled')
    const local = options.allowLoopbackForTests === true && isLoopback(url.hostname)
    if (
      url.username ||
      url.password ||
      url.hash ||
      (!local && url.protocol !== 'https:') ||
      (local && url.protocol !== 'http:' && url.protocol !== 'https:') ||
      (!local && privateLiteral(url.hostname))
    )
      throw badRequest('url', 'outbound_policy')
    if (!local) await options.checker.checkTarget(new URL(url.href), signal)
    if (signal.aborted) throw new KitFailure('cancelled', 'Target check cancelled')
  }
}
