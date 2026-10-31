import assert from 'node:assert/strict'
import { isRecord } from '../guard.js'
import type { OutboundHttp } from '../ports.js'
import type { McpCachedTool, McpConfigStore, McpServerConfig } from './index.js'
export const signal = () => new AbortController().signal
export const cached = (
  overrides: Partial<McpCachedTool> = {},
): McpCachedTool => ({
  name: 'echo',
  description: 'Echo',
  schema: { type: 'object' },
  readOnly: false,
  approval: 'ask',
  fetchedAt: Date.now(),
  ...overrides,
})
export const server = (
  overrides: Partial<McpServerConfig> = {},
): McpServerConfig => ({
  id: 'one',
  slug: 'one',
  name: 'Server one',
  url: 'https://mcp.example/rpc',
  enabled: true,
  trusted: true,
  allowPrivate: false,
  headers: { Authorization: 'Bearer private-value' },
  query: { key: 'private-query' },
  tools: [cached()],
  ...overrides,
})
export class Store implements McpConfigStore {
  replacements = 0
  configs: McpServerConfig[]
  constructor(configs: McpServerConfig[]) {
    this.configs = configs
  }
  async list(): Promise<readonly McpServerConfig[]> {
    return this.configs
  }
  async get(id: string): Promise<McpServerConfig | null> {
    return this.configs.find((config) => config.id === id) ?? null
  }
  async replaceTools(
    id: string,
    tools: readonly McpCachedTool[],
  ): Promise<void> {
    this.replacements++
    this.configs = this.configs.map((config) =>
      config.id === id ? { ...config, tools } : config,
    )
  }
}
interface Sent {
  readonly url: string
  readonly init: RequestInit
  readonly rpc: Record<string, unknown>
}
export class Wire implements OutboundHttp {
  readonly sent: Sent[] = []
  private readonly reply: (sent: Sent) => Response | Promise<Response>
  constructor(reply: (sent: Sent) => Response | Promise<Response>) {
    this.reply = reply
  }
  async fetch(url: string, init: RequestInit): Promise<Response> {
    assert.equal(typeof init.body, 'string')
    const rpc: unknown = JSON.parse(String(init.body))
    assert.ok(isRecord(rpc))
    const sent = { url, init, rpc }
    this.sent.push(sent)
    if (rpc.method === 'initialize')
      return json(
        rpc.id,
        { protocolVersion: '2025-03-26' },
        { 'Mcp-Session-Id': 'session-one' },
      )
    if (rpc.method === 'notifications/initialized')
      return new Response(null, { status: 202 })
    return this.reply(sent)
  }
}
export function json(
  id: unknown,
  result: unknown,
  headers: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id, result }), {
    headers: { 'content-type': 'application/json', ...headers },
  })
}
