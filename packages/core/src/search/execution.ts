import type { SearchPreferences } from '@ouraihub/aether-contracts'
import { KitFailure } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import { providerFor, targetUrl, validatePreferences } from './policy.js'

/** Execution-only snapshot. Never persist or log this object. */
export interface ResolvedSearchConfig {
  readonly provider: string
  readonly baseUrl: string
  readonly apiKeys: readonly string[]
  readonly basicAuthUsername?: string
  readonly preferences: SearchPreferences
}
/** Host DNS/all-address policy preflight without sending an HTTP request. */
export type CheckTarget = (url: string, signal: AbortSignal) => Promise<void>
export interface Execution {
  readonly signal: AbortSignal
  wait<T>(work: Promise<T>): Promise<T>
  request(url: string, init: RequestInit): Promise<Response>
  body(response: Response): Promise<string>
}
export async function execute<T>(
  cap: 'search' | 'fetch',
  config: ResolvedSearchConfig,
  outbound: OutboundHttp,
  signal: AbortSignal | undefined,
  action: (key: string | undefined, execution: Execution) => Promise<T>,
): Promise<T> {
  const { config: spec, spec: provider } = providerFor(cap, config.provider)
  validatePreferences(config.preferences)
  if (
    !Array.isArray(config.apiKeys) ||
    config.apiKeys.some((key: unknown) => typeof key !== 'string' || !key.trim() || key.length > 4096)
  )
    throw new KitFailure('needs_config', 'Invalid search credentials', { field: 'keys' })
  if (spec.keyMode === 'required' && !config.apiKeys.length)
    throw new KitFailure('needs_config', 'Search credentials required', { field: 'keys' })
  const keys = config.provider === 'fetch' ? [] : config.apiKeys
  if (config.provider !== 'fetch') targetUrl(config.baseUrl)
  const controller = new AbortController()
  const abort = () => controller.abort()
  signal?.addEventListener('abort', abort, { once: true })
  if (signal?.aborted) controller.abort()
  const timer = setTimeout(abort, cap === 'fetch' ? 20000 : provider.id === 'exa-mcp' ? 25000 : 15000)
  const aborted = () =>
    new KitFailure(
      signal?.aborted ? 'cancelled' : 'timeout',
      signal?.aborted ? 'Search cancelled' : 'Search deadline exceeded',
    )
  let disconnect: (() => void) | undefined
  function wait<TValue>(work: Promise<TValue>): Promise<TValue> {
    const { promise, resolve, reject } = Promise.withResolvers<TValue>()
    const onAbort = () => reject(aborted())
    controller.signal.addEventListener('abort', onAbort, { once: true })
    if (controller.signal.aborted) onAbort()
    work
      .then(resolve, reject)
      .finally(() => controller.signal.removeEventListener('abort', onAbort))
      .catch(() => {})
    return promise
  }
  const execution: Execution = {
    signal: controller.signal,
    wait,
    async request(url, init) {
      if (controller.signal.aborted) throw aborted()
      targetUrl(url)
      disconnect?.()
      const connection = new AbortController()
      const abortConnection = () => connection.abort()
      controller.signal.addEventListener('abort', abortConnection, { once: true })
      disconnect = () => {
        controller.signal.removeEventListener('abort', abortConnection)
        connection.abort()
      }
      const connectTimer = setTimeout(abortConnection, 10000)
      try {
        const { promise, resolve, reject } = Promise.withResolvers<Response>()
        const onAbort = () =>
          reject(controller.signal.aborted ? aborted() : new KitFailure('timeout', 'Search connection timed out'))
        connection.signal.addEventListener('abort', onAbort, { once: true })
        const request: RequestInit & { credentials: 'omit' } = {
          ...init,
          redirect: 'manual',
          credentials: 'omit',
          signal: connection.signal,
        }
        outbound
          .fetch(url, request)
          .then(resolve, reject)
          .finally(() => connection.signal.removeEventListener('abort', onAbort))
          .catch(() => {})
        return await wait(promise)
      } finally {
        clearTimeout(connectTimer)
      }
    },
    async body(response) {
      const limit = response.ok ? 2 * 1024 * 1024 : 8 * 1024
      const length = response.headers.get('content-length')
      if (length !== null && /^\d+$/u.test(length) && Number(length) > limit) {
        await response.body?.cancel().catch(() => {})
        if (!response.ok)
          throw new KitFailure('upstream', 'Search provider rejected request', { status: response.status })
        throw new KitFailure('upstream', 'Search response exceeds size limit', { limit })
      }
      const reader = response.body?.getReader()
      const decoder = new TextDecoder()
      let text = ''
      let size = 0
      try {
        if (reader)
          for (;;) {
            const { done, value } = await wait(reader.read())
            if (done) break
            size += value.byteLength
            if (size > limit) {
              await reader.cancel().catch(() => {})
              if (!response.ok)
                throw new KitFailure('upstream', 'Search provider rejected request', { status: response.status })
              throw new KitFailure('upstream', 'Search response exceeds size limit', { limit })
            }
            text += decoder.decode(value, { stream: true })
          }
        text += decoder.decode()
      } catch (error) {
        void reader?.cancel().catch(() => {})
        throw error
      } finally {
        reader?.releaseLock()
      }
      if (!response.ok)
        throw new KitFailure('upstream', 'Search provider rejected request', { status: response.status })
      return text
    },
  }
  try {
    for (let attempt = 0; attempt < Math.max(1, keys.length); attempt++) {
      if (controller.signal.aborted) throw aborted()
      try {
        return await wait(action(keys[attempt], execution))
      } catch (error) {
        if (controller.signal.aborted) throw aborted()
        if (
          error instanceof KitFailure &&
          [401, 403, 429].includes(Number(error.details?.status)) &&
          attempt + 1 < keys.length
        )
          continue
        if (error instanceof KitFailure) throw error
        throw new KitFailure('network', 'Search provider request failed')
      }
    }
    throw new KitFailure('upstream', 'Search provider request failed')
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener('abort', abort)
    controller.abort()
    disconnect?.()
  }
}
