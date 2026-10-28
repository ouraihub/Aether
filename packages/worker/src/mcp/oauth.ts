import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import type { McpServiceOptions } from './types.js'
import type { McpAuth } from './auth.js'
import {
  body,
  equal,
  escapeHtml,
  forbidden,
  hash,
  json,
  jsonBody,
  object,
  random,
  strings,
  validActor,
} from './common.js'

interface OAuthClient {
  id: string
  name: string
  redirectUris: readonly string[]
  createdAt: number
}
interface Authorization {
  client: OAuthClient
  redirectUri: string
  scopes: string[]
  challenge: string
  state: string
  resource: string
}
function client(value: unknown): value is OAuthClient {
  return (
    object(value) &&
    typeof value.id === 'string' &&
    typeof value.name === 'string' &&
    strings(value.redirectUris) &&
    typeof value.createdAt === 'number'
  )
}
function redirectAllowed(value: string): boolean {
  try {
    const url = new URL(value)
    return (
      !url.username &&
      !url.password &&
      !url.hash &&
      (url.protocol === 'https:' ||
        (url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)))
    )
  } catch {
    return false
  }
}
function redirect(target: string): Response {
  return new Response(null, { status: 302, headers: { Location: target, 'Cache-Control': 'no-store' } })
}

export class McpOAuth {
  readonly options: McpServiceOptions
  readonly auth: McpAuth
  constructor(options: McpServiceOptions, auth: McpAuth) {
    this.options = options
    this.auth = auth
  }
  async handle(request: Request): Promise<Response | null> {
    const url = new URL(request.url)
    const origin = this.options.origin
    const resource = `${origin}/mcp`
    if (url.pathname === '/.well-known/oauth-protected-resource' && request.method === 'GET')
      return json({
        resource,
        authorization_servers: [origin],
        bearer_methods_supported: ['header'],
        scopes_supported: this.options.scopes.map((scope) => scope.id),
      })
    if (url.pathname === '/.well-known/oauth-authorization-server' && request.method === 'GET')
      return json({
        issuer: origin,
        authorization_endpoint: `${origin}/oauth/authorize`,
        token_endpoint: `${origin}/oauth/token`,
        registration_endpoint: `${origin}/oauth/register`,
        response_types_supported: ['code'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
        code_challenge_methods_supported: ['S256'],
        token_endpoint_auth_methods_supported: ['none'],
        scopes_supported: this.options.scopes.map((scope) => scope.id),
      })
    if (url.pathname === '/oauth/register' && request.method === 'POST') return this.register(request)
    if (url.pathname === '/oauth/authorize' && (request.method === 'GET' || request.method === 'POST'))
      return this.authorize(request)
    if (url.pathname === '/oauth/token' && request.method === 'POST') {
      try {
        return await this.token(request)
      } catch (error) {
        if (!(error instanceof KitFailure)) throw error
        return json({ error: error.code === 'bad_request' ? 'invalid_request' : 'invalid_grant' }, 400)
      }
    }
    if (url.pathname.startsWith('/oauth/') || url.pathname.startsWith('/.well-known/oauth-'))
      return json({ error: 'invalid_request' }, 400)
    return null
  }
  private async register(request: Request): Promise<Response> {
    const input = await jsonBody(request, 16384)
    if (
      !strings(input.redirect_uris) ||
      input.redirect_uris.length === 0 ||
      input.redirect_uris.length > 10 ||
      input.redirect_uris.some((uri) => uri.length > 2048 || !redirectAllowed(uri))
    )
      throw badRequest('redirect_uris', 'invalid')
    if (input.token_endpoint_auth_method !== undefined && input.token_endpoint_auth_method !== 'none')
      throw badRequest('token_endpoint_auth_method', 'unsupported')
    if (
      input.grant_types !== undefined &&
      (!strings(input.grant_types) ||
        input.grant_types.some((type) => type !== 'authorization_code' && type !== 'refresh_token'))
    )
      throw badRequest('grant_types', 'unsupported')
    if (
      input.response_types !== undefined &&
      (!strings(input.response_types) || input.response_types.some((type) => type !== 'code'))
    )
      throw badRequest('response_types', 'unsupported')
    if (input.client_name !== undefined && (typeof input.client_name !== 'string' || input.client_name.length > 128))
      throw badRequest('client_name', 'invalid')
    const record: OAuthClient = {
      id: crypto.randomUUID(),
      name: typeof input.client_name === 'string' ? input.client_name : 'MCP client',
      redirectUris: [...new Set(input.redirect_uris)],
      createdAt: this.auth.now(),
    }
    await this.options.store.compareAndSet(this.auth.system, 'mcp-clients', record.id, null, record)
    return json(
      {
        client_id: record.id,
        client_name: record.name,
        redirect_uris: record.redirectUris,
        token_endpoint_auth_method: 'none',
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
      },
      201,
    )
  }
  private async parse(request: Request): Promise<Authorization> {
    const query = new URL(request.url).searchParams
    for (const key of [
      'client_id',
      'redirect_uri',
      'response_type',
      'scope',
      'code_challenge',
      'code_challenge_method',
      'state',
      'resource',
    ])
      if (query.getAll(key).length > 1) throw badRequest(key, 'duplicate')
    const record = await this.options.store.get(this.auth.system, 'mcp-clients', query.get('client_id') ?? '')
    if (!record || !client(record.value)) throw badRequest('client_id', 'invalid')
    const redirectUri = query.get('redirect_uri') ?? ''
    if (!record.value.redirectUris.includes(redirectUri)) throw badRequest('redirect_uri', 'mismatch')
    if (query.get('response_type') !== 'code' || query.get('code_challenge_method') !== 'S256')
      throw badRequest('response_type', 'unsupported')
    const challenge = query.get('code_challenge') ?? ''
    if (!/^[A-Za-z0-9_-]{43}$/u.test(challenge)) throw badRequest('code_challenge', 'invalid')
    const resource = `${this.options.origin}/mcp`
    if (query.has('resource') && query.get('resource') !== resource) throw badRequest('resource', 'invalid')
    const scopes = this.auth.requested((query.get('scope') ?? '').split(/\s+/u).filter(Boolean))
    return { client: record.value, redirectUri, scopes, challenge, state: query.get('state') ?? '', resource }
  }
  private async authorize(request: Request): Promise<Response> {
    const parsed = await this.parse(request)
    const actor = await this.options.resolveIdentity(request)
    if (!actor || !validActor(actor, this.options.applicationId) || actor.kind !== 'user')
      return json({ error: 'login_required' }, 401)
    this.auth.actor(actor)
    this.auth.requested(parsed.scopes, actor)
    const settings = await this.auth.settings(actor)
    if (!settings.enabled || parsed.scopes.some((scope) => !settings.enabledScopes.includes(scope))) forbidden()
    const requestHash = await hash(new URL(request.url).searchParams.toString())
    if (request.method === 'GET') {
      const csrf = random()
      await this.options.store.compareAndSet(actor, 'mcp-consent', await hash(csrf), null, {
        requestHash,
        sessionId: actor.sessionId,
        expiresAt: this.auth.now() + 600000,
        used: false,
      })
      const scopeLabels = parsed.scopes.map(
        (scope) => this.options.scopes.find((item) => item.id === scope)?.label ?? scope,
      )
      const html = `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Authorize MCP client</title><body><main><h1>Authorize ${escapeHtml(parsed.client.name)}</h1><p>Grant the following permissions?</p><ul>${scopeLabels.map((label) => `<li>${escapeHtml(label)}</li>`).join('')}</ul><form method="post" action="${escapeHtml(new URL(request.url).pathname + new URL(request.url).search)}"><input type="hidden" name="csrf" value="${csrf}"><button name="decision" value="approve">Approve</button><button name="decision" value="deny">Deny</button></form></main></body></html>`
      return new Response(html, {
        headers: {
          'Content-Type': 'text/html; charset=utf-8',
          'Cache-Control': 'no-store',
          'Content-Security-Policy': "default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
          'Referrer-Policy': 'no-referrer',
          'X-Content-Type-Options': 'nosniff',
        },
      })
    }
    if (request.headers.get('Origin') !== this.options.origin) forbidden()
    const form = await this.form(request)
    const csrf = form.get('csrf') ?? ''
    const document = await this.options.store.get(actor, 'mcp-consent', await hash(csrf))
    if (
      !document ||
      !object(document.value) ||
      document.value.used !== false ||
      document.value.requestHash !== requestHash ||
      document.value.sessionId !== actor.sessionId ||
      typeof document.value.expiresAt !== 'number' ||
      document.value.expiresAt <= this.auth.now()
    )
      forbidden()
    await this.options.store.compareAndSet(actor, 'mcp-consent', document.key, document.revision, {
      ...document.value,
      used: true,
    })
    const target = new URL(parsed.redirectUri)
    if (parsed.state) target.searchParams.set('state', parsed.state)
    if (form.get('decision') !== 'approve' || !(await this.options.authorizeConsent(actor, parsed.scopes, request))) {
      target.searchParams.set('error', 'access_denied')
      return redirect(target.toString())
    }
    const grant = await this.auth.createGrant(actor, parsed.client.id, parsed.redirectUri, parsed.scopes)
    const code = random()
    await this.options.store.compareAndSet(this.auth.system, 'mcp-codes', await hash(code), null, {
      owner: actor,
      clientId: parsed.client.id,
      redirectUri: parsed.redirectUri,
      challenge: parsed.challenge,
      grantId: grant.id,
      expiresAt: this.auth.now() + 120000,
      used: false,
      resource: parsed.resource,
    })
    target.searchParams.set('code', code)
    return redirect(target.toString())
  }
  private async form(request: Request): Promise<URLSearchParams> {
    if (!(request.headers.get('Content-Type') ?? '').toLowerCase().startsWith('application/x-www-form-urlencoded'))
      throw badRequest('body', 'form_required')
    const form = new URLSearchParams(await body(request, 16384))
    for (const key of new Set(form.keys())) if (form.getAll(key).length !== 1) throw badRequest(key, 'duplicate')
    return form
  }
  private async token(request: Request): Promise<Response> {
    const form = await this.form(request)
    const clientId = form.get('client_id') ?? ''
    if (form.has('resource') && form.get('resource') !== `${this.options.origin}/mcp`)
      throw badRequest('resource', 'invalid')
    if (form.get('grant_type') === 'refresh_token')
      return json(await this.auth.refresh(form.get('refresh_token') ?? '', clientId))
    if (form.get('grant_type') !== 'authorization_code') throw badRequest('grant_type', 'unsupported')
    const document = await this.options.store.get(this.auth.system, 'mcp-codes', await hash(form.get('code') ?? ''))
    if (!document || !object(document.value)) throw new KitFailure('unauthorized', 'Invalid grant')
    const value = document.value
    const verifier = form.get('code_verifier') ?? ''
    if (
      value.resource !== `${this.options.origin}/mcp` ||
      !validActor(value.owner, this.options.applicationId) ||
      value.owner.kind !== 'user' ||
      value.used !== false ||
      typeof value.expiresAt !== 'number' ||
      value.expiresAt <= this.auth.now() ||
      value.clientId !== clientId ||
      value.redirectUri !== form.get('redirect_uri') ||
      typeof value.challenge !== 'string' ||
      typeof value.grantId !== 'string' ||
      !/^[A-Za-z0-9._~-]{43,128}$/u.test(verifier) ||
      !equal(await hash(verifier), value.challenge)
    )
      throw new KitFailure('unauthorized', 'Invalid grant')
    await this.options.store.compareAndSet(this.auth.system, 'mcp-codes', document.key, document.revision, {
      ...value,
      used: true,
    })
    return json(await this.auth.issueTokens(value.owner, value.grantId, undefined, clientId))
  }
}
