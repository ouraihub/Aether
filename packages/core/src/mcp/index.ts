import { KitFailure, badRequest, cancelled } from '../error.js'
import { isRecord as record } from '../guard.js'
import type { OutboundHttp } from '../ports.js'
import {
  abortFailure,
  abortable,
  budget,
  encoder,
  invalid,
  MAX_MESSAGE_BYTES,
  parseMcpCallResult,
  normalize,
  readReply,
  reason,
  RequestFailure,
  RpcFailure,
} from './protocol.js'
import type {
  McpCachedTool,
  McpCallResult,
  McpConfigStore,
  McpServerConfig,
  McpTool,
} from './types.js'

export type {
  Approval,
  McpCachedTool,
  McpCallResult,
  McpConfigStore,
  McpServerConfig,
  McpTool,
} from './types.js'
export { MAX_MESSAGE_BYTES, parseMcpCallResult } from './protocol.js'

const MAX_SCHEMA_BYTES = 32 * 1024
const CACHE_AGE_MS = 10 * 60 * 1000

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
    const timer = setTimeout(
      () => controller.abort(new KitFailure('timeout', 'MCP initialization timed out')),
      10_000,
    )
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
      const result = await this.request(
        'tools/list',
        cursor === undefined ? {} : { cursor },
        signal,
      )
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
          description:
            typeof tool.description === 'string'
              ? [...tool.description].slice(0, 1024).join('')
              : '',
          schema: tool.inputSchema,
          readOnly: record(tool.annotations) && tool.annotations.readOnlyHint === true,
          approval: old.get(tool.name) ?? 'ask',
          fetchedAt: Date.now(),
        })
      }
      if (!('nextCursor' in result)) return tools
      if (
        typeof result.nextCursor !== 'string' ||
        !result.nextCursor ||
        cursors.has(result.nextCursor)
      )
        throw invalid()
      cursors.add(result.nextCursor)
      cursor = result.nextCursor
    }
    throw budget()
  }

  async call(
    name: string,
    args: Record<string, unknown>,
    signal: AbortSignal,
  ): Promise<McpCallResult> {
    try {
      await this.initialize(signal)
    } catch (error) {
      const failure = normalize(error, signal)
      return { kind: 'failed', code: failure.code, message: failure.message }
    }
    try {
      return parseMcpCallResult(await this.request('tools/call', { name, arguments: args }, signal))
    } catch (error) {
      if (error instanceof RpcFailure)
        return { kind: 'failed', code: String(error.rpcCode), message: error.message }
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
    const timer = setTimeout(
      () => controller.abort(new KitFailure('timeout', 'MCP request timed out')),
      60_000,
    )
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
            host === 'localhost' ||
            host.endsWith('.localhost') ||
            host.endsWith('.local') ||
            !host.includes('.')
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
        this.outbound.fetch(url.toString(), {
          method: 'POST',
          headers,
          body,
          signal: combined,
          redirect: 'error',
        }),
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
  async call(
    prefixed: string,
    args: Record<string, unknown>,
    signal: AbortSignal,
  ): Promise<McpCallResult> {
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
    const task = this.track(
      this.backgroundTail.catch(() => {}).then(() => this.refresh(id, this.closing.signal)),
    )
    this.backgroundTail = task
    this.refreshing.set(id, task)
    void task.then(
      () => this.refreshing.delete(id),
      () => this.refreshing.delete(id),
    )
  }

  session(signal: AbortSignal): Promise<McpSession> {
    if (signal.aborted || this.closing.signal.aborted)
      return Promise.resolve(new McpSession([], this.closing.signal))
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
        if (config.tools.some((tool) => Date.now() - tool.fetchedAt > CACHE_AGE_MS))
          this.background(config.id)
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
