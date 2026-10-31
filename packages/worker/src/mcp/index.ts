import { KitFailure, safeError, badRequest } from '@ouraihub/aether-core/error'
import type { WorkerContext } from '../context.js'
import { McpClientSettings } from './client.js'
import { McpAuth } from './auth.js'
import { McpOAuth } from './oauth.js'
import {
  body,
  forbidden,
  json,
  jsonBody,
  object,
  validActor,
} from './common.js'
import type { Authentication } from './auth.js'
import type { ExposedMcpTool, McpServiceOptions } from './types.js'
export type {
  ApiKeyRecord,
  GrantRecord,
  McpScope,
  McpSettings,
  ExposedMcpTool,
  McpCallContext,
  McpServiceOptions,
} from './types.js'
export { McpClientSettings } from './client.js'

const MAX_RESULT_BYTES = 2 * 1024 * 1024
const encoder = new TextEncoder()
function rpc(id: unknown, result: unknown, error = false): Response {
  const message = {
    jsonrpc: '2.0',
    id: typeof id === 'string' || typeof id === 'number' ? id : null,
    ...(error ? { error: result } : { result }),
  }
  if (encoder.encode(JSON.stringify(message)).byteLength > MAX_RESULT_BYTES)
    return json({
      jsonrpc: '2.0',
      id: message.id,
      error: { code: -32603, message: 'Result budget exceeded' },
    })
  return json(message)
}
function rpcError(id: unknown, code: number, message: string): Response {
  return rpc(id, { code, message }, true)
}

export class McpService {
  private readonly auth: McpAuth
  private readonly oauth: McpOAuth
  private readonly clients: McpClientSettings
  readonly options: McpServiceOptions
  constructor(options: McpServiceOptions) {
    this.options = options
    const origin = new URL(options.origin)
    if (
      origin.origin !== options.origin ||
      origin.protocol !== 'https:' ||
      !options.applicationId
    )
      throw badRequest('origin', 'https_origin_required')
    const scopeIds = new Set<string>()
    for (const scope of options.scopes) {
      if (
        !/^[a-zA-Z0-9][a-zA-Z0-9:_.-]{0,127}$/u.test(scope.id) ||
        scopeIds.has(scope.id)
      )
        throw badRequest('scopes', 'invalid_registration')
      scopeIds.add(scope.id)
    }
    const tools = new Set<string>()
    for (const tool of options.tools) {
      if (
        !/^[A-Za-z0-9_.-]{1,128}$/u.test(tool.name) ||
        tools.has(tool.name) ||
        !scopeIds.has(tool.scope) ||
        !object(tool.inputSchema) ||
        !Number.isSafeInteger(tool.maxRequestBytes ?? 65536) ||
        (tool.maxRequestBytes ?? 65536) < 1 ||
        (tool.maxRequestBytes ?? 65536) > MAX_RESULT_BYTES
      )
        throw badRequest('tools', 'invalid_registration')
      tools.add(tool.name)
    }
    this.auth = new McpAuth(options)
    this.oauth = new McpOAuth(options, this.auth)
    this.clients = new McpClientSettings(
      options.store,
      options.credentials,
      options.outbound,
    )
  }
  client(context: WorkerContext) {
    if (!validActor(context.actor, this.options.applicationId)) forbidden()
    return this.clients.runtime(context.actor)
  }
  async handlePublic(request: Request): Promise<Response | null> {
    const url = new URL(request.url)
    if (
      url.pathname !== '/mcp' &&
      !url.pathname.startsWith('/oauth/') &&
      !url.pathname.startsWith('/.well-known/oauth-')
    )
      return null
    if (url.origin !== this.options.origin || url.protocol !== 'https:')
      return json({ error: 'invalid_request' }, 400)
    if (url.pathname === '/mcp') return this.mcp(request)
    return this.oauth.handle(request)
  }
  async handle(
    request: Request,
    context: WorkerContext,
  ): Promise<Response | null> {
    const path = new URL(request.url).pathname
    if (!path.startsWith('/api/v1/mcp')) return null
    this.auth.actor(context.actor)
    const clientResponse = await this.clients.handle(request, context)
    if (clientResponse) return clientResponse
    if (path === '/api/v1/mcp/settings') {
      if (request.method === 'GET')
        return json({
          ...(await this.auth.settings(context.actor)),
          scopes: this.options.scopes,
        })
      if (request.method === 'PUT') {
        const value = await jsonBody(request)
        if (
          !Array.isArray(value.enabledScopes) ||
          !value.enabledScopes.every(
            (scope: unknown) => typeof scope === 'string',
          )
        )
          throw badRequest('enabledScopes', 'invalid')
        const requested = value.enabledScopes.length
          ? this.auth.requested(value.enabledScopes, context.actor)
          : []
        if (
          !(await this.options.authorizeConsent(
            context.actor,
            requested,
            request,
          ))
        )
          forbidden()
        return json({
          ...(await this.auth.saveSettings(context.actor, value)),
          scopes: this.options.scopes,
        })
      }
    }
    if (path === '/api/v1/mcp/grants' && request.method === 'GET')
      return json(await this.auth.grants(context.actor))
    if (path === '/api/v1/mcp/api-keys') {
      if (request.method === 'GET')
        return json(await this.auth.keys(context.actor))
      if (request.method === 'POST')
        return json(
          await this.auth.issueKey(
            context.actor,
            await jsonBody(request),
            request,
          ),
          201,
        )
    }
    const revoke = /^\/api\/v1\/mcp\/(grants|api-keys)\/([^/]+)\/revoke$/u.exec(
      path,
    )
    if (revoke && request.method === 'POST') {
      const id = revoke[2]
      if (!id) throw badRequest('id', 'invalid')
      await this.auth.revoke(
        context.actor,
        revoke[1] === 'grants' ? 'grant' : 'key',
        decodeURIComponent(id),
      )
      return json({ ok: true })
    }
    return json({ code: 'not_found', message: 'Route not found' }, 404)
  }
  private async mcp(request: Request): Promise<Response> {
    const origin = request.headers.get('Origin')
    if (
      origin !== null &&
      ![this.options.origin, ...(this.options.mcpOrigins ?? [])].includes(
        origin,
      )
    )
      return json({ error: 'forbidden' }, 403)
    if (request.method !== 'POST')
      return new Response(null, { status: 405, headers: { Allow: 'POST' } })
    const bearer = /^Bearer[\t ]+([^\s,]+)$/iu.exec(
      request.headers.get('Authorization') ?? '',
    )?.[1]
    const authentication = bearer ? await this.auth.authenticate(bearer) : null
    if (!authentication)
      return json({ error: 'unauthorized' }, 401, {
        'WWW-Authenticate': `Bearer resource_metadata="${this.options.origin}/.well-known/oauth-protected-resource"`,
      })
    if (
      this.options.scopes.some((scope) => scope.id === 'mcp:access') &&
      !authentication.scopes.includes('mcp:access')
    )
      return json({ error: 'forbidden' }, 403)
    const maxBytes =
      this.options.tools.reduce(
        (max, tool) => Math.max(max, tool.maxRequestBytes ?? 65536),
        65536,
      ) + 8192
    let message: unknown
    try {
      message = JSON.parse(await body(request, maxBytes))
    } catch (error) {
      if (error instanceof KitFailure && error.code === 'bad_request')
        return json(
          { code: 'bad_request', message: 'Invalid request body' },
          413,
        )
      return rpcError(null, -32700, 'Parse error')
    }
    if (
      !object(message) ||
      message.jsonrpc !== '2.0' ||
      typeof message.method !== 'string' ||
      ('id' in message &&
        typeof message.id !== 'string' &&
        typeof message.id !== 'number' &&
        message.id !== null)
    )
      return rpcError(null, -32600, 'Invalid request')
    if (!('id' in message)) {
      if (
        message.method === 'notifications/initialized' ||
        message.method === 'notifications/cancelled'
      )
        return new Response(null, { status: 202 })
      return rpcError(null, -32600, 'Invalid request')
    }
    const id = message.id
    if (message.method === 'initialize') {
      if (
        !object(message.params) ||
        !['2024-11-05', '2025-03-26'].includes(
          String(message.params.protocolVersion),
        )
      )
        return rpcError(id, -32602, 'Unsupported protocol version')
      return rpc(id, {
        protocolVersion: message.params.protocolVersion,
        capabilities: { tools: {} },
        serverInfo: { name: 'Aether MCP', version: '1.0.0' },
      })
    }
    const protocol = request.headers.get('MCP-Protocol-Version')
    if (protocol !== null && !['2024-11-05', '2025-03-26'].includes(protocol))
      return rpcError(id, -32602, 'Unsupported protocol version')
    if (message.method === 'ping') return rpc(id, {})
    if (message.method === 'tools/list')
      return rpc(id, {
        tools: this.options.tools
          .filter((tool) => authentication.scopes.includes(tool.scope))
          .map((tool) => ({
            name: tool.name,
            description: tool.description,
            inputSchema: tool.inputSchema,
            annotations: {
              readOnlyHint: tool.readOnly,
              destructiveHint: tool.destructive,
              idempotentHint: tool.idempotent,
            },
          })),
      })
    if (message.method !== 'tools/call')
      return rpcError(id, -32601, 'Method not found')
    if (!object(message.params) || typeof message.params.name !== 'string')
      return rpcError(id, -32602, 'Invalid parameters')
    const toolName = message.params.name
    const tool = this.options.tools.find((item) => item.name === toolName)
    if (!tool) return rpcError(id, -32004, 'Tool not found')
    if (!authentication.scopes.includes(tool.scope))
      return rpcError(id, -32001, 'Forbidden')
    const args: unknown =
      'arguments' in message.params ? message.params.arguments : {}
    if (!object(args)) return rpcError(id, -32602, 'Invalid parameters')
    if (
      encoder.encode(JSON.stringify(args)).byteLength >
      (tool.maxRequestBytes ?? 65536)
    )
      return rpcError(id, -32602, 'Arguments too large')
    if (!tool.validate(args)) return rpcError(id, -32602, 'Invalid parameters')
    return this.call(id, tool, args, authentication, request.signal)
  }
  private async call(
    id: unknown,
    tool: ExposedMcpTool,
    args: Record<string, unknown>,
    auth: Authentication,
    signal: AbortSignal,
  ): Promise<Response> {
    try {
      if (
        signal.aborted ||
        !(await this.options.authorizeTool(auth.actor, tool, args, signal))
      )
        return rpcError(id, -32001, 'Forbidden')
      if (signal.aborted) return rpcError(id, -32603, 'Cancelled')
      const result = await tool.handler(args, {
        actor: auth.actor,
        tokenId: auth.tokenId,
        scopes: auth.scopes,
        signal,
      })
      if (result === undefined)
        throw new KitFailure('internal', 'Invalid tool result')
      const payload = object(result)
        ? {
            content: [{ type: 'text', text: JSON.stringify(result) }],
            structuredContent: result,
            isError: false,
          }
        : {
            content: [
              {
                type: 'text',
                text:
                  typeof result === 'string' ? result : JSON.stringify(result),
              },
            ],
            isError: false,
          }
      const serialized = JSON.stringify({ jsonrpc: '2.0', id, result: payload })
      if (encoder.encode(serialized).byteLength > MAX_RESULT_BYTES)
        return rpcError(id, -32603, 'Result budget exceeded')
      return new Response(serialized, {
        headers: {
          'Content-Type': 'application/json',
          'Cache-Control': 'no-store',
        },
      })
    } catch (error) {
      const safe = safeError(error)
      const code =
        safe.code === 'bad_request'
          ? -32602
          : safe.code === 'not_found'
            ? -32004
            : safe.code === 'conflict'
              ? -32009
              : -32603
      return rpc(id, { code, message: safe.message, data: safe }, true)
    }
  }
}
export function createMcpService(options: McpServiceOptions): McpService {
  return new McpService(options)
}
