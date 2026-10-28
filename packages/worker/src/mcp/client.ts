import type { ServerDto, ToolDto } from '@ouraihub/aether-contracts'
import { badRequest, KitFailure } from '@ouraihub/aether-core/error'
import { isRecord as object } from '@ouraihub/aether-core/guard'
import {
  McpRuntime,
  type Approval,
  type McpCachedTool,
  type McpConfigStore,
  type McpServerConfig,
} from '@ouraihub/aether-core/mcp'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import { ownerKey, type Actor, type WorkerContext } from '../context.js'
import { isSealedCredential, type CredentialPort, type CredentialScope, type SealedCredential } from '../credentials.js'
import type { DocumentStore } from '../storage.js'

const COLLECTION = 'mcp-client'
const KEY = 'servers'
const SECRET_PARAMS: Readonly<Record<string, true>> = {
  key: true,
  api_key: true,
  apikey: true,
  token: true,
  access_token: true,
  ak: true,
  sk: true,
  secret: true,
  client_secret: true,
  password: true,
  signature: true,
}
const SECRET_PATTERN =
  /(?:\b(?:sk-|ghp_|gho_|github_pat_|xoxb-|xoxp-|AKIA|AIza|glpat-)[A-Za-z0-9_./+=-]{0,}|\bBearer\s+[A-Za-z0-9_./+=-]{20,}|-----BEGIN[^\r\n]*PRIVATE KEY)/g

interface Secret {
  headers: Record<string, string>
  query: Record<string, string>
}
interface Server {
  id: string
  slug: string
  name: string
  url: string
  enabled: boolean
  trusted: boolean
  allowPrivate: boolean
  secret: SealedCredential | null
  secretKeys: string[]
  tools: McpCachedTool[]
}
interface State {
  version: 1
  servers: Server[]
  deleted: string[]
}
interface Snapshot {
  revision: number | null
  state: State
}

function fields(value: object, allowed: readonly string[], field: string): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) throw badRequest(field, 'invalid')
}
function text(value: unknown, field: string): string {
  if (typeof value !== 'string' || !value.trim()) throw badRequest(field, 'required')
  return value
}
function bool(value: unknown, field: string, fallback?: boolean): boolean {
  if (value === undefined && fallback !== undefined) return fallback
  if (typeof value !== 'boolean') throw badRequest(field, 'invalid')
  return value
}
function approval(value: unknown): Approval {
  if (value !== 'auto' && value !== 'ask' && value !== 'deny') throw badRequest('approval', 'invalid')
  return value
}
function looksSecret(value: string): boolean {
  for (const match of value.matchAll(SECRET_PATTERN))
    if (match[0].length >= 20 || match[0].startsWith('-----BEGIN')) return true
  return false
}
function serverUrl(raw: unknown, allowPrivate: boolean): string {
  const input = text(raw, 'url')
  let url: URL
  try {
    url = new URL(input)
  } catch {
    throw badRequest('url', 'invalid_url')
  }
  if (url.username || url.password) throw badRequest('url', 'userinfo')
  if (url.protocol !== 'https:' && url.protocol !== 'http:') throw badRequest('url', 'scheme')
  if (!url.hostname) throw badRequest('url', 'invalid_url')
  if (url.hash) throw badRequest('url', 'fragment')
  for (const [name, value] of url.searchParams) {
    if (Object.hasOwn(SECRET_PARAMS, name.toLowerCase()) || looksSecret(value))
      throw badRequest('url', 'credential_query')
  }
  const host = url.hostname.replace(/^\[|\]$/g, '')
  const loopback = host === 'localhost' || host.endsWith('.localhost') || host === '::1' || /^127\./.test(host)
  if (url.protocol === 'http:' && !allowPrivate) throw badRequest('url', 'plain_http')
  // DNS and connection-time restrictions remain the injected outbound port's responsibility.
  const restricted =
    /^(?:0\.|10\.|169\.254\.|192\.168\.|172\.(?:1[6-9]|2\d|3[01])\.|198\.(?:18|19)\.|(?:22[4-9]|23\d|24\d|25[0-5])\.)/.test(
      host,
    ) ||
    host === '::' ||
    /^(?:f[cd][0-9a-f]{2}:|fe[89ab][0-9a-f]:|ff[0-9a-f]{2}:)/i.test(host)
  if ((restricted || loopback) && !allowPrivate) throw badRequest('url', 'blocked_address')
  return url.href
}
function maskedUrl(raw: string): string {
  const url = new URL(raw)
  url.username = ''
  url.password = ''
  for (const [name, value] of url.searchParams)
    if (Object.hasOwn(SECRET_PARAMS, name.toLowerCase()) || looksSecret(value)) url.searchParams.set(name, '***')
  return url.href
}
function strings(value: unknown, field: string): Record<string, string> {
  if (value === undefined) return {}
  if (!object(value)) throw badRequest(field, 'invalid')
  const result: Record<string, string> = {}
  for (const [key, entry] of Object.entries(value)) {
    if (!key || typeof entry !== 'string') throw badRequest(field, 'invalid')
    Object.defineProperty(result, key, { value: entry, enumerable: true, writable: true, configurable: true })
  }
  return result
}
function parseSecret(value: unknown): Secret {
  if (!object(value)) throw badRequest('secret', 'invalid')
  fields(value, ['headers', 'query', 'env'], 'secret')
  if (value.env !== undefined && Object.keys(strings(value.env, 'secret')).length)
    throw badRequest('secret', 'transport_mismatch')
  const headers = strings(value.headers, 'secret.headers')
  const query = strings(value.query, 'secret.query')
  for (const [name, entry] of Object.entries(headers)) {
    if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name) || /[^\t\x20-\x7e\x80-\xff]/.test(entry))
      throw badRequest('headers', 'invalid')
  }
  return { headers, query }
}
function cached(value: unknown): McpCachedTool {
  if (!object(value)) throw badRequest('tools', 'invalid')
  fields(value, ['name', 'description', 'schema', 'readOnly', 'approval', 'fetchedAt'], 'tools')
  if (
    typeof value.description !== 'string' ||
    !object(value.schema) ||
    typeof value.fetchedAt !== 'number' ||
    !Number.isSafeInteger(value.fetchedAt) ||
    value.fetchedAt < 0
  )
    throw badRequest('tools', 'invalid')
  if ([...value.description].length > 1024) throw badRequest('tools', 'too_large')
  if (new TextEncoder().encode(JSON.stringify(value.schema)).length > 32768) throw badRequest('tools', 'too_large')
  return {
    name: text(value.name, 'tool'),
    description: value.description,
    schema: value.schema,
    readOnly: bool(value.readOnly, 'readOnly'),
    approval: approval(value.approval),
    fetchedAt: value.fetchedAt,
  }
}
function toolList(value: unknown): McpCachedTool[] {
  if (!Array.isArray(value) || value.length > 200) throw badRequest('tools', 'invalid')
  const tools = value.map(cached)
  if (new Set(tools.map((tool) => tool.name)).size !== tools.length) throw badRequest('tools', 'duplicate')
  return tools
}
function readState(value: unknown): State {
  try {
    if (!object(value) || value.version !== 1 || !Array.isArray(value.servers) || !Array.isArray(value.deleted))
      throw badRequest('mcp', 'invalid')
    fields(value, ['version', 'servers', 'deleted'], 'mcp')
    const deleted = value.deleted.map((id: unknown) => text(id, 'id'))
    const servers = value.servers.map((entry: unknown): Server => {
      if (!object(entry)) throw badRequest('mcp', 'invalid')
      fields(
        entry,
        ['id', 'slug', 'name', 'url', 'enabled', 'trusted', 'allowPrivate', 'secret', 'secretKeys', 'tools'],
        'mcp',
      )
      const slug = text(entry.slug, 'slug')
      if (
        !/^[a-z0-9_]{1,24}$/.test(slug) ||
        !Array.isArray(entry.secretKeys) ||
        !(entry.secret === null || isSealedCredential(entry.secret))
      )
        throw badRequest('mcp', 'invalid')
      if (entry.secret !== null) fields(entry.secret, ['version', 'keyId', 'nonce', 'ciphertext'], 'secret')
      if (
        new Set(entry.secretKeys).size !== entry.secretKeys.length ||
        (entry.secret !== null && !entry.secretKeys.length)
      )
        throw badRequest('mcp', 'invalid')
      const allowPrivate = bool(entry.allowPrivate, 'allowPrivate')
      return {
        id: text(entry.id, 'id'),
        slug,
        name: text(entry.name, 'name'),
        url: serverUrl(entry.url, allowPrivate),
        enabled: bool(entry.enabled, 'enabled'),
        trusted: bool(entry.trusted, 'trusted'),
        allowPrivate,
        secret: entry.secret,
        secretKeys: entry.secretKeys.map((key: unknown) => text(key, 'secretKeys')),
        tools: toolList(entry.tools),
      }
    })
    if (
      new Set(servers.map((server) => server.id)).size !== servers.length ||
      new Set(servers.map((server) => server.slug)).size !== servers.length ||
      new Set(deleted).size !== deleted.length ||
      servers.some(
        (server) => deleted.includes(server.id) || (server.secret === null && server.secretKeys.length !== 0),
      )
    )
      throw badRequest('mcp', 'invalid')
    return { version: 1, servers, deleted }
  } catch {
    throw new KitFailure('internal', 'Invalid stored MCP configuration')
  }
}
function toolDto(tool: McpCachedTool): ToolDto {
  return { name: tool.name, description: tool.description, readOnly: tool.readOnly, approval: tool.approval }
}
function dto(server: Server): ServerDto {
  return {
    id: server.id,
    slug: server.slug,
    name: server.name,
    transport: { kind: 'http', url: maskedUrl(server.url) },
    enabled: server.enabled,
    trusted: server.trusted,
    allowPrivate: server.allowPrivate,
    secretKeys: server.secretKeys,
    tools: server.tools.map(toolDto),
  }
}
function missing(): KitFailure {
  return new KitFailure('not_found', 'MCP server or tool not found')
}
async function body(request: Request): Promise<Record<string, unknown>> {
  let value: unknown
  try {
    value = await request.json()
  } catch {
    throw badRequest('body', 'invalid_json')
  }
  if (!object(value)) throw badRequest('body', 'invalid')
  return value
}

export class McpClientSettings {
  private readonly runtimes = new Map<string, McpRuntime>()
  private readonly store: DocumentStore
  private readonly credentials: CredentialPort
  private readonly outbound: OutboundHttp
  constructor(store: DocumentStore, credentials: CredentialPort, outbound: OutboundHttp) {
    this.store = store
    this.credentials = credentials
    this.outbound = outbound
  }

  private scope(actor: Actor, id: string): CredentialScope {
    return {
      applicationId: actor.applicationId,
      tenantId: actor.tenantId,
      subjectId: actor.subjectId,
      purpose: 'mcp',
      recordId: id,
    }
  }
  private async snapshot(actor: Actor): Promise<Snapshot> {
    const document = await this.store.get(actor, COLLECTION, KEY)
    return document
      ? { revision: document.revision, state: readState(document.value) }
      : { revision: null, state: { version: 1, servers: [], deleted: [] } }
  }
  private async commit(actor: Actor, snapshot: Snapshot): Promise<void> {
    await this.store.compareAndSet(actor, COLLECTION, KEY, snapshot.revision, snapshot.state)
  }
  private async config(actor: Actor, server: Server): Promise<McpServerConfig> {
    let secret: Secret = { headers: {}, query: {} }
    if (server.secret) {
      let bytes: Uint8Array | undefined
      try {
        bytes = await this.credentials.open(this.scope(actor, server.id), server.secret)
        const value: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
        secret = parseSecret(value)
      } catch {
        throw new KitFailure('needs_config', 'MCP credentials are unreadable', { reason: 'key_unreadable' })
      } finally {
        bytes?.fill(0)
      }
    }
    return {
      id: server.id,
      slug: server.slug,
      name: server.name,
      url: server.url,
      enabled: server.enabled,
      trusted: server.trusted,
      allowPrivate: server.allowPrivate,
      headers: secret.headers,
      query: secret.query,
      tools: server.tools,
    }
  }
  runtime(actor: Actor): McpRuntime {
    const key = ownerKey(actor)
    const existing = this.runtimes.get(key)
    if (existing) return existing
    const owner: Actor = { ...actor, scopes: [...actor.scopes] }
    const configs: McpConfigStore = {
      list: async () => {
        const { state } = await this.snapshot(owner)
        return Promise.all(state.servers.map((server) => this.config(owner, server)))
      },
      get: async (id) => {
        const { state } = await this.snapshot(owner)
        const server = state.servers.find((entry) => entry.id === id)
        return server ? this.config(owner, server) : null
      },
      replaceTools: async (id, tools) => {
        const snapshot = await this.snapshot(owner)
        const server = snapshot.state.servers.find((entry) => entry.id === id)
        if (!server) throw missing()
        const approvals = new Map(server.tools.map((tool) => [tool.name, tool.approval]))
        server.tools = toolList(tools).map((tool) => ({ ...tool, approval: approvals.get(tool.name) ?? 'ask' }))
        await this.commit(owner, snapshot)
      },
    }
    const runtime = new McpRuntime(this.outbound, configs)
    this.runtimes.set(key, runtime)
    return runtime
  }
  private async save(actor: Actor, input: Record<string, unknown>, id?: string): Promise<ServerDto[]> {
    fields(input, ['id', 'slug', 'name', 'transport', 'secret', 'enabled', 'trusted', 'allowPrivate'], 'body')
    if (input.id !== undefined && input.id !== id) throw badRequest('id', 'mismatch')
    const snapshot = await this.snapshot(actor)
    const existing = id === undefined ? undefined : snapshot.state.servers.find((server) => server.id === id)
    if (id !== undefined && !existing) throw missing()
    const slug = text(input.slug, 'slug')
    if (!/^[a-z0-9_]{1,24}$/.test(slug)) throw badRequest('slug', 'invalid')
    if (snapshot.state.servers.some((server) => server.slug === slug && server.id !== id))
      throw new KitFailure('conflict', 'MCP slug already exists', { reason: 'duplicate_slug', field: 'slug' })
    if (!object(input.transport)) throw badRequest('transport', 'invalid')
    if (input.transport.kind === 'stdio') throw new KitFailure('unsupported', 'Worker MCP clients support HTTP only')
    fields(input.transport, ['kind', 'url'], 'transport')
    if (input.transport.kind !== 'http') throw badRequest('transport', 'invalid')
    const allowPrivate = bool(input.allowPrivate, 'allowPrivate', false)
    const server: Server = {
      id: existing?.id ?? crypto.randomUUID(),
      slug,
      name: text(input.name, 'name'),
      url: serverUrl(input.transport.url, allowPrivate),
      enabled: bool(input.enabled, 'enabled', false),
      trusted: bool(input.trusted, 'trusted', false),
      allowPrivate,
      secret: existing?.secret ?? null,
      secretKeys: existing?.secretKeys ?? [],
      tools: existing?.tools ?? [],
    }
    if (input.secret !== undefined) {
      const secret = parseSecret(input.secret)
      server.secretKeys = [...new Set([...Object.keys(secret.headers), ...Object.keys(secret.query)])].sort()
      if (server.secretKeys.length) {
        const bytes = new TextEncoder().encode(JSON.stringify(secret))
        try {
          server.secret = await this.credentials.seal(this.scope(actor, server.id), bytes)
        } finally {
          bytes.fill(0)
        }
      } else server.secret = null
    }
    if (existing) snapshot.state.servers[snapshot.state.servers.indexOf(existing)] = server
    else snapshot.state.servers.push(server)
    await this.commit(actor, snapshot)
    return snapshot.state.servers.map(dto)
  }
  async handle(request: Request, context: WorkerContext): Promise<Response | null> {
    const path = new URL(request.url).pathname
    const match = /^\/api\/v1\/mcp\/servers(?:\/([^/]+)(?:\/(refresh)|\/tools\/([^/]+)\/(approval))?)?$/.exec(path)
    if (!match) return null
    let id: string | undefined
    let tool: string | undefined
    try {
      id = match[1] === undefined ? undefined : decodeURIComponent(match[1])
      tool = match[3] === undefined ? undefined : decodeURIComponent(match[3])
    } catch {
      throw badRequest('path', 'invalid')
    }
    const actor = context.actor
    if (!id && request.method === 'GET') return Response.json((await this.snapshot(actor)).state.servers.map(dto))
    if (!id && request.method === 'POST') return Response.json(await this.save(actor, await body(request)))
    if (id && !match[2] && !match[4] && request.method === 'PUT')
      return Response.json(await this.save(actor, await body(request), id))
    if (id && !match[2] && !match[4] && request.method === 'DELETE') {
      const snapshot = await this.snapshot(actor)
      if (!snapshot.state.servers.some((server) => server.id === id)) throw missing()
      snapshot.state.servers = snapshot.state.servers.filter((server) => server.id !== id)
      snapshot.state.deleted.push(id)
      await this.commit(actor, snapshot)
      return Response.json(snapshot.state.servers.map(dto))
    }
    if (id && match[2] && request.method === 'POST') {
      await this.runtime(actor).refresh(id, context.signal)
      const server = (await this.snapshot(actor)).state.servers.find((entry) => entry.id === id)
      if (!server) throw missing()
      return Response.json(server.tools.map(toolDto))
    }
    if (id && tool && match[4] && request.method === 'PUT') {
      const input = await body(request)
      fields(input, ['approval'], 'body')
      const value = approval(input.approval)
      const snapshot = await this.snapshot(actor)
      const server = snapshot.state.servers.find((entry) => entry.id === id)
      const cached = server?.tools.find((entry) => entry.name === tool)
      if (!server || !cached) throw missing()
      server.tools = server.tools.map((entry) => (entry === cached ? { ...entry, approval: value } : entry))
      await this.commit(actor, snapshot)
      return Response.json({ ok: true })
    }
    throw badRequest('method', 'unsupported')
  }
}
