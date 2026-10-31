const CACHE_AGE_MS = 10 * 60 * 1000

import { KitFailure, cancelled } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import type {
  McpCachedTool,
  McpCallResult,
  McpConfigStore,
  McpServerConfig,
  McpTool,
} from './types.js'
import { HttpConnection } from './connection.js'
import { abortable, abortFailure, normalize } from './protocol.js'
export type * from './types.js'
export { MAX_MESSAGE_BYTES, parseMcpCallResult } from './protocol.js'

interface SessionServer {
  readonly config: McpServerConfig
  readonly connection: HttpConnection
}
export class McpSession {
  private readonly catalogue: readonly McpTool[]
  private readonly calls = new Map<
    string,
    { connection: HttpConnection; name: string }
  >()
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
    if (combined.aborted)
      return { kind: 'failed', code: 'cancelled', message: 'Cancelled' }
    const target = this.calls.get(prefixed)
    if (!target)
      return {
        kind: 'failed',
        code: 'not_found',
        message: 'MCP tool not found',
      }
    return target.connection.call(target.name, args, combined)
  }
}

export class McpRuntime {
  private readonly closing = new AbortController()
  private readonly tasks = new Set<Promise<unknown>>()
  private readonly refreshing = new Map<
    string,
    Promise<readonly McpCachedTool[]>
  >()
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
    if (this.closing.signal.aborted || signal.aborted)
      return Promise.reject(cancelled())
    const combined = AbortSignal.any([signal, this.closing.signal])
    return this.track(this.refreshConfig(id, combined))
  }

  private async refreshConfig(
    id: string,
    signal: AbortSignal,
  ): Promise<readonly McpCachedTool[]> {
    try {
      const stored = await abortable(this.store.get(id), signal)
      if (!stored) throw new KitFailure('not_found', 'MCP server not found')
      const config = structuredClone(stored)
      const tools = await new HttpConnection(this.outbound, config).discover(
        signal,
      )
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
      this.backgroundTail
        .catch(() => {})
        .then(() => this.refresh(id, this.closing.signal)),
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
        if (
          config.tools.some(
            (tool) => Date.now() - tool.fetchedAt > CACHE_AGE_MS,
          )
        )
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
