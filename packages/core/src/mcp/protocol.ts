import { KitFailure, cancelled } from '../error.js'
import { isRecord as record } from '../guard.js'
import { SseParser } from '../sse.js'
import type { SseEvent } from '../sse.js'
import type { McpCallResult } from './types.js'
export const MAX_MESSAGE_BYTES = 2 * 1024 * 1024
export const MAX_SCHEMA_BYTES = 32 * 1024
export const encoder = new TextEncoder()
export const invalid = (): KitFailure =>
  new KitFailure('upstream', 'Invalid MCP response', {
    reason: 'invalid_response',
  })
export const budget = (): KitFailure =>
  new KitFailure('upstream', 'MCP message limit exceeded', {
    reason: 'budget_exceeded',
  })
export const abortFailure = (signal: AbortSignal): KitFailure =>
  signal.reason instanceof KitFailure ? signal.reason : cancelled()

export function abortable<T>(
  operation: Promise<T>,
  signal: AbortSignal,
): Promise<T> {
  const { promise, resolve, reject } = Promise.withResolvers<T>()
  const abort = () => {
    signal.removeEventListener('abort', abort)
    reject(abortFailure(signal))
  }
  if (signal.aborted) reject(abortFailure(signal))
  else signal.addEventListener('abort', abort, { once: true })
  operation.then(
    (value) => {
      signal.removeEventListener('abort', abort)
      resolve(value)
    },
    (error) => {
      signal.removeEventListener('abort', abort)
      reject(error)
    },
  )
  return promise
}

export class RequestFailure extends Error {
  readonly failure: KitFailure
  readonly dispatched: boolean
  readonly reason: string
  readonly expired: boolean
  constructor(
    failure: KitFailure,
    dispatched: boolean,
    reason: string,
    expired = false,
  ) {
    super(failure.message)
    this.failure = failure
    this.dispatched = dispatched
    this.reason = reason
    this.expired = expired
  }
}
export class RpcFailure extends KitFailure {
  readonly rpcCode: number
  constructor(rpcCode: number) {
    super('upstream', 'MCP server rejected the request', {
      reason: 'rpc_error',
    })
    this.rpcCode = rpcCode
  }
}
export function normalize(error: unknown, signal: AbortSignal): KitFailure {
  if (signal.aborted) return abortFailure(signal)
  if (error instanceof RequestFailure) return error.failure
  return error instanceof KitFailure
    ? error
    : new KitFailure('network', 'MCP connection failed')
}
export function reason(error: KitFailure): string {
  const detail = error.details?.reason
  return typeof detail === 'string' &&
    ['invalid_response', 'budget_exceeded'].includes(detail)
    ? detail
    : error.code === 'cancelled'
      ? 'cancelled'
      : error.code === 'timeout'
        ? 'timeout'
        : 'network'
}

function envelope(value: unknown, id: number): Record<string, unknown> {
  if (
    !record(value) ||
    value.jsonrpc !== '2.0' ||
    value.id !== id ||
    'result' in value === 'error' in value
  )
    throw invalid()
  if ('error' in value) {
    if (
      !record(value.error) ||
      !Number.isInteger(value.error.code) ||
      typeof value.error.code !== 'number' ||
      typeof value.error.message !== 'string'
    )
      throw invalid()
    throw new RpcFailure(value.error.code)
  }
  if (!record(value.result)) throw invalid()
  return value.result
}

/** Validates without flattening nontext content or interpreting extension fields. */
export function parseMcpCallResult(value: unknown): McpCallResult {
  if (
    !record(value) ||
    !Array.isArray(value.content) ||
    ('isError' in value && typeof value.isError !== 'boolean') ||
    ('structuredContent' in value && !record(value.structuredContent))
  )
    return { kind: 'unknown', reason: 'invalid_response' }
  const content: Record<string, unknown>[] = []
  for (const item of value.content) {
    if (
      !record(item) ||
      typeof item.type !== 'string' ||
      !item.type.trim() ||
      (item.type === 'text' && typeof item.text !== 'string')
    )
      return { kind: 'unknown', reason: 'invalid_response' }
    content.push(item)
  }
  return {
    kind: 'completed',
    content,
    isError: value.isError === true,
    ...(record(value.structuredContent)
      ? { structuredContent: value.structuredContent }
      : {}),
  }
}

export async function readReply(
  response: Response,
  id: number,
  signal: AbortSignal,
): Promise<Record<string, unknown>> {
  const sse =
    response.headers
      .get('content-type')
      ?.split(';')[0]
      ?.trim()
      .toLowerCase() === 'text/event-stream'
  const contentType = response.headers
    .get('content-type')
    ?.split(';')[0]
    ?.trim()
    .toLowerCase()
  if (!sse && contentType !== 'application/json') throw invalid()
  if (!response.body) throw invalid()
  const reader = response.body.getReader()
  const parser = sse
    ? new SseParser(MAX_MESSAGE_BYTES, MAX_MESSAGE_BYTES)
    : null
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: false })
  let text = ''
  let bytes = 0
  const eventsReply = (
    events: readonly { data: string }[],
  ): Record<string, unknown> | null => {
    for (const event of events) {
      if (encoder.encode(event.data).byteLength > MAX_MESSAGE_BYTES)
        throw budget()
      let value: unknown
      try {
        value = JSON.parse(event.data)
      } catch {
        throw invalid()
      }
      if (!record(value) || value.id !== id) continue
      return envelope(value, id)
    }
    return null
  }
  try {
    for (;;) {
      const chunk = await abortable(reader.read(), signal)
      if (chunk.done) break
      if (parser) {
        let events: SseEvent[]
        try {
          events = parser.feed(chunk.value)
        } catch (error) {
          if (
            error instanceof KitFailure &&
            error.details?.reason === 'sse_limit'
          )
            throw budget()
          throw invalid()
        }
        const result = eventsReply(events)
        if (result) return result
      } else {
        bytes += chunk.value.byteLength
        if (bytes > MAX_MESSAGE_BYTES) throw budget()
        try {
          text += decoder.decode(chunk.value, { stream: true })
        } catch {
          throw invalid()
        }
      }
    }
    if (parser) {
      let events: SseEvent[]
      try {
        events = parser.finish()
      } catch (error) {
        if (
          error instanceof KitFailure &&
          error.details?.reason === 'sse_limit'
        )
          throw budget()
        throw invalid()
      }
      const result = eventsReply(events)
      if (result) return result
      throw invalid()
    }
    try {
      text += decoder.decode()
    } catch {
      throw invalid()
    }
    let value: unknown
    try {
      value = JSON.parse(text)
    } catch {
      throw invalid()
    }
    return envelope(value, id)
  } finally {
    void reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}
