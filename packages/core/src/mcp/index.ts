import { KitFailure, badRequest, cancelled } from '../error.js'
import { isRecord as record } from '../guard.js'
import type { OutboundHttp } from '../ports.js'
import { SseParser } from '../sse.js'
import type { SseEvent } from '../sse.js'

export type Approval = 'auto' | 'ask' | 'deny'
export interface McpTool {
  readonly server: string
  readonly slug: string
  readonly name: string
  readonly prefixed: string
  readonly description: string
  readonly schema: Readonly<Record<string, unknown>>
  readonly approval: Approval
}
export type McpCallResult =
  | {
      readonly kind: 'completed'
      readonly content: readonly Record<string, unknown>[]
      readonly structuredContent?: Record<string, unknown>
      readonly isError: boolean
    }
  | { readonly kind: 'failed'; readonly code: string; readonly message: string }
  | { readonly kind: 'unknown'; readonly reason: string }
export interface McpCachedTool {
  readonly name: string
  readonly description: string
  readonly schema: Readonly<Record<string, unknown>>
  readonly readOnly: boolean
  readonly approval: Approval
  readonly fetchedAt: number
}
export interface McpServerConfig {
  readonly id: string
  readonly slug: string
  readonly name: string
  readonly url: string
  readonly enabled: boolean
  readonly trusted: boolean
  readonly allowPrivate: boolean
  readonly headers: Readonly<Record<string, string>>
  readonly query: Readonly<Record<string, string>>
  readonly tools: readonly McpCachedTool[]
}
export interface McpConfigStore {
  list(): Promise<readonly McpServerConfig[]>
  get(id: string): Promise<McpServerConfig | null>
  replaceTools(id: string, tools: readonly McpCachedTool[]): Promise<void>
}

export const MAX_MESSAGE_BYTES = 2 * 1024 * 1024
const MAX_SCHEMA_BYTES = 32 * 1024
const CACHE_AGE_MS = 10 * 60 * 1000
const encoder = new TextEncoder()
const invalid = (): KitFailure => new KitFailure('upstream', 'Invalid MCP response', { reason: 'invalid_response' })
const budget = (): KitFailure => new KitFailure('upstream', 'MCP message limit exceeded', { reason: 'budget_exceeded' })
const abortFailure = (signal: AbortSignal): KitFailure =>
  signal.reason instanceof KitFailure ? signal.reason : cancelled()

function abortable<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
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

class RequestFailure extends Error {
  readonly failure: KitFailure
  readonly dispatched: boolean
  readonly reason: string
  readonly expired: boolean
  constructor(failure: KitFailure, dispatched: boolean, reason: string, expired = false) {
    super(failure.message)
    this.failure = failure
    this.dispatched = dispatched
    this.reason = reason
    this.expired = expired
  }
}
class RpcFailure extends KitFailure {
  readonly rpcCode: number
  constructor(rpcCode: number) {
    super('upstream', 'MCP server rejected the request', { reason: 'rpc_error' })
    this.rpcCode = rpcCode
  }
}
function normalize(error: unknown, signal: AbortSignal): KitFailure {
  if (signal.aborted) return abortFailure(signal)
  if (error instanceof RequestFailure) return error.failure
  return error instanceof KitFailure ? error : new KitFailure('network', 'MCP connection failed')
}
function reason(error: KitFailure): string {
  const detail = error.details?.reason
  return typeof detail === 'string' && ['invalid_response', 'budget_exceeded'].includes(detail)
    ? detail
    : error.code === 'cancelled'
      ? 'cancelled'
      : error.code === 'timeout'
        ? 'timeout'
        : 'network'
}

function envelope(value: unknown, id: number): Record<string, unknown> {
  if (!record(value) || value.jsonrpc !== '2.0' || value.id !== id || 'result' in value === 'error' in value)
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
    ...(record(value.structuredContent) ? { structuredContent: value.structuredContent } : {}),
  }
}

async function readReply(response: Response, id: number, signal: AbortSignal): Promise<Record<string, unknown>> {
  const sse = response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() === 'text/event-stream'
  const contentType = response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase()
  if (!sse && contentType !== 'application/json') throw invalid()
  if (!response.body) throw invalid()
  const reader = response.body.getReader()
  const parser = sse ? new SseParser(MAX_MESSAGE_BYTES, MAX_MESSAGE_BYTES) : null
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: false })
  let text = ''
  let bytes = 0
  const eventsReply = (events: readonly { data: string }[]): Record<string, unknown> | null => {
    for (const event of events) {
      if (encoder.encode(event.data).byteLength > MAX_MESSAGE_BYTES) throw budget()
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
          if (error instanceof KitFailure && error.details?.reason === 'sse_limit') throw budget()
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
        if (error instanceof KitFailure && error.details?.reason === 'sse_limit') throw budget()
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

class HttpConnection {
  private sessionId: string | null = null
  private version: string | null = null
  private nextId = 1
  private initialized: Promise<void> | null = null
  private readonly outbound: OutboundHttp
  private readonly config: McpServerConfig
  constructor(outbound: OutboundHttp, config: McpServerConfig) {
    this.outbound = outbound
    this.config = config
  }

  private reset(): void {
    this.sessionId = null
    this.version = null
    this.initialized = null
  }

  async initialize(signal: AbortSignal): Promise<void> {
    if (signal.aborted) throw abortFailure(signal)
    if (!this.initialized) {
      const initializing = this.handshake(signal)
      this.initialized = initializing
      void initializing.catch(() => {
        if (this.initialized === initializing) this.reset()
      })
    }
    return abortable(this.initialized, signal)
  }

  private async handshake(signal: AbortSignal): Promise<void> {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(new KitFailure('timeout', 'MCP initialization timed out')), 10_000)
    const combined = AbortSignal.any([signal, controller.signal])
    try {
      const result = await this.request(
        'initialize',
        {
          protocolVersion: '2025-03-26',
          capabilities: {},
          clientInfo: { name: 'aether', version: '1.0.0' },
        },
        combined,
      )
      if (typeof result.protocolVersion !== 'string') throw invalid()
      if (result.protocolVersion !== '2024-11-05' && result.protocolVersion !== '2025-03-26')
        throw new KitFailure('unsupported', 'Unsupported MCP protocol version')
      this.version = result.protocolVersion
      await this.request('notifications/initialized', undefined, combined, true)
    } finally {
      clearTimeout(timer)
    }
  }

  async discover(signal: AbortSignal): Promise<readonly McpCachedTool[]> {
    for (let attempt = 0; ; attempt++) {
      try {
        await this.initialize(signal)
        return await this.list(signal)
      } catch (error) {
        if (attempt === 0 && error instanceof RequestFailure && error.expired) {
          this.reset()
          continue
        }
        throw error
      }
    }
  }

  private async list(signal: AbortSignal): Promise<readonly McpCachedTool[]> {
    const tools: McpCachedTool[] = []
    const names = new Set<string>()
    const cursors = new Set<string>()
    const old = new Map(this.config.tools.map((tool) => [tool.name, tool.approval]))
    let cursor: string | undefined
    for (let page = 0; page < 10; page++) {
      const result = await this.request('tools/list', cursor === undefined ? {} : { cursor }, signal)
      if (!Array.isArray(result.tools)) throw invalid()
      for (const tool of result.tools) {
        if (
          !record(tool) ||
          typeof tool.name !== 'string' ||
          !tool.name ||
          !record(tool.inputSchema) ||
          ('description' in tool && typeof tool.description !== 'string') ||
          ('annotations' in tool && !record(tool.annotations))
        )
          throw invalid()
        if (names.has(tool.name)) throw invalid()
        names.add(tool.name)
        if (names.size > 200) throw budget()
        if (encoder.encode(JSON.stringify(tool.inputSchema)).byteLength > MAX_SCHEMA_BYTES) continue
        tools.push({
          name: tool.name,
          description: typeof tool.description === 'string' ? [...tool.description].slice(0, 1024).join('') : '',
          schema: tool.inputSchema,
          readOnly: record(tool.annotations) && tool.annotations.readOnlyHint === true,
          approval: old.get(tool.name) ?? 'ask',
          fetchedAt: Date.now(),
        })
      }
      if (!('nextCursor' in result)) return tools
      if (typeof result.nextCursor !== 'string' || !result.nextCursor || cursors.has(result.nextCursor)) throw invalid()
      cursors.add(result.nextCursor)
      cursor = result.nextCursor
    }
    throw budget()
  }

  async call(name: string, args: Record<string, unknown>, signal: AbortSignal): Promise<McpCallResult> {
    try {
      await this.initialize(signal)
    } catch (error) {
      const failure = normalize(error, signal)
      return { kind: 'failed', code: failure.code, message: failure.message }
    }
    try {
      return parseMcpCallResult(await this.request('tools/call', { name, arguments: args }, signal))
    } catch (error) {
      if (error instanceof RpcFailure) return { kind: 'failed', code: String(error.rpcCode), message: error.message }
      if (error instanceof RequestFailure && error.dispatched) {
        if (error.expired) this.reset()
        return { kind: 'unknown', reason: error.reason }
      }
      const failure = normalize(error, signal)
      return { kind: 'failed', code: failure.code, message: failure.message }
    }
  }

  private async request(
    method: string,
    params: Record<string, unknown> | undefined,
    signal: AbortSignal,
    notification = false,
  ): Promise<Record<string, unknown>> {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(new KitFailure('timeout', 'MCP request timed out')), 60_000)
    const combined = AbortSignal.any([signal, controller.signal])
    let dispatched = false
    let expired = false
    try {
      if (combined.aborted) throw abortFailure(combined)
      let url: URL
      try {
        url = new URL(this.config.url)
      } catch {
        throw badRequest('url', 'invalid_url')
      }
      if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash)
        throw badRequest('url', 'invalid_url')
      if (!this.config.allowPrivate) {
        if (url.protocol !== 'https:') throw badRequest('url', 'plain_http')
        const host = url.hostname
        let restricted: boolean
        if (host.startsWith('[')) {
          const ip = host.slice(1, -1).toLowerCase()
          restricted =
            !/^[23][0-9a-f]{0,3}:/u.test(ip) ||
            ip.startsWith('2001:db8:') ||
            ip.startsWith('2001:0:') ||
            ip.startsWith('2002:')
        } else if (/^\d+(?:\.\d+){3}$/u.test(host)) {
          const [a = 0, b = 0, c = 0] = host.split('.').map(Number)
          restricted =
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
        } else
          restricted =
            host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local') || !host.includes('.')
        if (restricted) throw badRequest('url', 'blocked_address')
      }
      for (const [key, value] of Object.entries(this.config.query)) url.searchParams.set(key, value)
      const headers = new Headers(this.config.headers)
      headers.set('content-type', 'application/json')
      headers.set('accept', 'application/json, text/event-stream')
      if (this.sessionId) headers.set('Mcp-Session-Id', this.sessionId)
      if (this.version) headers.set('MCP-Protocol-Version', this.version)
      const id = this.nextId++
      let body: string
      try {
        body = JSON.stringify({
          jsonrpc: '2.0',
          ...(notification ? {} : { id }),
          method,
          ...(params ? { params } : {}),
        })
      } catch {
        throw badRequest('arguments', 'not_json')
      }
      if (encoder.encode(body).byteLength > MAX_MESSAGE_BYTES)
        throw badRequest('arguments', 'too_large', MAX_MESSAGE_BYTES)
      if (combined.aborted) throw abortFailure(combined)
      dispatched = true
      const response = await abortable(
        this.outbound.fetch(url.toString(), { method: 'POST', headers, body, signal: combined, redirect: 'error' }),
        combined,
      )
      if (!response.ok) {
        expired = response.status === 404 && this.sessionId !== null
        void response.body?.cancel().catch(() => {})
        throw new KitFailure(
          response.status === 401 || response.status === 403 ? 'unauthorized' : 'upstream',
          'MCP HTTP request failed',
        )
      }
      const sessionId = response.headers.get('Mcp-Session-Id')
      if (sessionId !== null) {
        if (!/^[\x21-\x7e]+$/.test(sessionId)) throw invalid()
        this.sessionId = sessionId
      }
      if (notification) {
        void response.body?.cancel().catch(() => {})
        return {}
      }
      return await readReply(response, id, combined)
    } catch (error) {
      if (error instanceof RpcFailure) throw error
      const failure = normalize(error, combined)
      throw new RequestFailure(failure, dispatched, reason(failure), expired)
    } finally {
      clearTimeout(timer)
    }
  }
}

interface SessionServer {
  readonly config: McpServerConfig
  readonly connection: HttpConnection
}
export class McpSession {
  private readonly catalogue: readonly McpTool[]
  private readonly calls = new Map<string, { connection: HttpConnection; name: string }>()
  private readonly shutdownSignal: AbortSignal
  constructor(servers: readonly SessionServer[], shutdownSignal: AbortSignal) {
    this.shutdownSignal = shutdownSignal
    const catalogue: McpTool[] = []
    for (const { config, connection } of servers) {
      for (const tool of config.tools) {
        const prefixed = `mcp_${config.slug}_${tool.name}`
        catalogue.push({
          server: config.name,
          slug: config.slug,
          name: tool.name,
          prefixed,
          description: tool.description,
          schema: tool.schema,
          approval: tool.approval,
        })
        this.calls.set(prefixed, { connection, name: tool.name })
      }
    }
    this.catalogue = catalogue
  }
  tools(): readonly McpTool[] {
    return this.catalogue
  }
  async call(prefixed: string, args: Record<string, unknown>, signal: AbortSignal): Promise<McpCallResult> {
    const combined = AbortSignal.any([signal, this.shutdownSignal])
    if (combined.aborted) return { kind: 'failed', code: 'cancelled', message: 'Cancelled' }
    const target = this.calls.get(prefixed)
    if (!target) return { kind: 'failed', code: 'not_found', message: 'MCP tool not found' }
    return target.connection.call(target.name, args, combined)
  }
}

export class McpRuntime {
  private readonly closing = new AbortController()
  private readonly tasks = new Set<Promise<unknown>>()
  private readonly refreshing = new Map<string, Promise<readonly McpCachedTool[]>>()
  private drain: Promise<void> | null = null
  private readonly outbound: OutboundHttp
  private readonly store: McpConfigStore
  private backgroundTail: Promise<unknown> = Promise.resolve()
  constructor(outbound: OutboundHttp, store: McpConfigStore) {
    this.outbound = outbound
    this.store = store
  }

  private track<T>(task: Promise<T>): Promise<T> {
    this.tasks.add(task)
    void task.then(
      () => this.tasks.delete(task),
      () => this.tasks.delete(task),
    )
    return task
  }

  refresh(id: string, signal: AbortSignal): Promise<readonly McpCachedTool[]> {
    if (this.closing.signal.aborted || signal.aborted) return Promise.reject(cancelled())
    const combined = AbortSignal.any([signal, this.closing.signal])
    return this.track(this.refreshConfig(id, combined))
  }

  private async refreshConfig(id: string, signal: AbortSignal): Promise<readonly McpCachedTool[]> {
    try {
      const stored = await abortable(this.store.get(id), signal)
      if (!stored) throw new KitFailure('not_found', 'MCP server not found')
      const config = structuredClone(stored)
      const tools = await new HttpConnection(this.outbound, config).discover(signal)
      if (signal.aborted) throw abortFailure(signal)
      await this.store.replaceTools(id, tools)
      if (signal.aborted) throw abortFailure(signal)
      return tools
    } catch (error) {
      throw normalize(error, signal)
    }
  }

  private background(id: string): void {
    if (this.closing.signal.aborted || this.refreshing.has(id)) return
    const task = this.track(this.backgroundTail.catch(() => {}).then(() => this.refresh(id, this.closing.signal)))
    this.backgroundTail = task
    this.refreshing.set(id, task)
    void task.then(
      () => this.refreshing.delete(id),
      () => this.refreshing.delete(id),
    )
  }

  session(signal: AbortSignal): Promise<McpSession> {
    if (signal.aborted || this.closing.signal.aborted) return Promise.resolve(new McpSession([], this.closing.signal))
    const combined = AbortSignal.any([signal, this.closing.signal])
    return this.track(this.prepare(combined))
  }

  private async prepare(signal: AbortSignal): Promise<McpSession> {
    const servers: SessionServer[] = []
    let configs: readonly McpServerConfig[]
    try {
      configs = structuredClone(await abortable(this.store.list(), signal))
    } catch {
      return new McpSession([], this.closing.signal)
    }
    for (const config of configs) {
      if (signal.aborted) return new McpSession([], this.closing.signal)
      if (!config.enabled || !config.trusted) continue
      const connection = new HttpConnection(this.outbound, config)
      if (config.tools.length) {
        servers.push({ config, connection })
        if (config.tools.some((tool) => Date.now() - tool.fetchedAt > CACHE_AGE_MS)) this.background(config.id)
      } else {
        try {
          const tools = await connection.discover(signal)
          if (signal.aborted) return new McpSession([], this.closing.signal)
          await this.store.replaceTools(config.id, tools)
          if (signal.aborted) return new McpSession([], this.closing.signal)
          servers.push({ config: { ...config, tools }, connection })
        } catch {
          /* One unavailable server must not hide the rest of the catalogue. */
        }
      }
    }
    return new McpSession(servers, this.closing.signal)
  }

  shutdown(): Promise<void> {
    if (!this.drain) {
      this.closing.abort(cancelled())
      this.drain = Promise.allSettled([...this.tasks]).then(() => {})
    }
    return this.drain
  }
}
