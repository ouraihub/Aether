import assert from 'node:assert/strict'
import { test } from 'node:test'
import { KitFailure } from '@ouraihub/aether-core/error'
import type { Actor, WorkerContext } from '../context.js'
import { ownerKey } from '../context.js'
import type { DocumentStore, StoredDocument } from '../storage.js'
import { McpService } from './index.js'
import type { McpServiceOptions } from './types.js'
import { hash, object } from './common.js'

class TestDocuments implements DocumentStore {
  readonly records = new Map<string, StoredDocument>()
  private key(actor: Actor, collection: string, key: string): string {
    return JSON.stringify([ownerKey(actor), collection, key])
  }
  async get(
    actor: Actor,
    collection: string,
    key: string,
  ): Promise<StoredDocument | null> {
    return this.records.get(this.key(actor, collection, key)) ?? null
  }
  async list(
    actor: Actor,
    collection: string,
  ): Promise<readonly StoredDocument[]> {
    const prefix =
      JSON.stringify([ownerKey(actor), collection]).slice(0, -1) + ','
    return [...this.records]
      .filter(([key]) => key.startsWith(prefix))
      .map(([, value]) => value)
  }
  async compareAndSet(
    actor: Actor,
    collection: string,
    key: string,
    revision: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    const address = this.key(actor, collection, key)
    const previous = this.records.get(address)
    if ((previous?.revision ?? null) !== revision)
      throw new KitFailure('conflict', 'CAS conflict')
    const document = {
      key,
      revision: (previous?.revision ?? 0) + 1,
      value: structuredClone(value),
    }
    this.records.set(address, document)
    return document
  }
  async remove(
    actor: Actor,
    collection: string,
    key: string,
    revision: number,
  ): Promise<void> {
    const address = this.key(actor, collection, key)
    if (this.records.get(address)?.revision !== revision)
      throw new KitFailure('conflict', 'CAS conflict')
    this.records.delete(address)
  }
}
const origin = 'https://mcp.example'
const actor: Actor = {
  applicationId: 'app',
  subjectId: 'owner',
  tenantId: 'team',
  sessionId: 'session',
  kind: 'user',
  scopes: ['mcp:access', 'files:read', 'files:write'],
}
function context(owner: Actor = actor): WorkerContext {
  return {
    actor: owner,
    signal: new AbortController().signal,
    requestId: 'test',
    now: 1000,
  }
}
function fixture(overrides: Partial<McpServiceOptions> = {}) {
  const store = new TestDocuments()
  let calls = 0
  const service = new McpService({
    applicationId: 'app',
    origin,
    store,
    credentials: {
      seal: async () => {
        throw new Error('unused')
      },
      open: async () => {
        throw new Error('unused')
      },
      rotate: async () => {
        throw new Error('unused')
      },
    },
    outbound: {
      fetch: async () => {
        throw new Error('unused')
      },
    },
    scopes: [
      { id: 'mcp:access', label: 'MCP', destructive: false },
      { id: 'files:read', label: 'Read', destructive: false },
      { id: 'files:write', label: 'Write', destructive: true },
    ],
    tools: [
      {
        name: 'read_file',
        description: 'Read file',
        inputSchema: { type: 'object' },
        scope: 'files:read',
        readOnly: true,
        destructive: false,
        idempotent: true,
        validate: (value) => typeof value.path === 'string',
        handler: async () => {
          calls++
          return { value: 'read' }
        },
      },
      {
        name: 'write_file',
        description: 'Write file',
        inputSchema: { type: 'object' },
        scope: 'files:write',
        readOnly: false,
        destructive: true,
        idempotent: false,
        validate: () => true,
        handler: async () => {
          calls++
          return 'written'
        },
      },
    ],
    resolveIdentity: async () => actor,
    authorizeConsent: async () => true,
    authorizeTool: async () => true,
    now: () => 1000,
    ...overrides,
  })
  return { service, store, calls: () => calls }
}
function request(
  path: string,
  method = 'GET',
  data?: unknown,
  token?: string,
): Request {
  const headers = new Headers({ Origin: origin })
  if (data !== undefined) headers.set('Content-Type', 'application/json')
  if (token) headers.set('Authorization', `Bearer ${token}`)
  return new Request(`${origin}${path}`, {
    method,
    headers,
    ...(data === undefined ? {} : { body: JSON.stringify(data) }),
  })
}
async function value(
  response: Response | null,
): Promise<Record<string, unknown>> {
  assert.ok(response)
  const result: unknown = await response.json()
  assert.ok(object(result))
  return result
}
async function enable(service: McpService): Promise<void> {
  await service.handle(
    request('/api/v1/mcp/settings', 'PUT', {
      enabled: true,
      enabledScopes: actor.scopes,
    }),
    context(),
  )
}
async function key(service: McpService, scopes = ['mcp:access', 'files:read']) {
  const issued = await value(
    await service.handle(
      request('/api/v1/mcp/api-keys', 'POST', { name: 'client', scopes }),
      context(),
    ),
  )
  assert.equal(typeof issued.token, 'string')
  assert.ok(typeof issued.token === 'string')
  assert.ok(object(issued.record) && typeof issued.record.id === 'string')
  return { token: issued.token, id: issued.record.id }
}
async function rpc(
  service: McpService,
  token: string,
  method: string,
  params: unknown = {},
) {
  return value(
    await service.handlePublic(
      request('/mcp', 'POST', { jsonrpc: '2.0', id: 1, method, params }, token),
    ),
  )
}

test('API keys reveal once; scope directory and call checks agree; revocation immediate', async () => {
  const { service, store, calls } = fixture()
  await enable(service)
  const issued = await key(service)
  assert.ok(!JSON.stringify([...store.records.values()]).includes(issued.token))
  const listed = await service.handle(
    request('/api/v1/mcp/api-keys'),
    context(),
  )
  assert.ok(listed)
  assert.ok(!(await listed.text()).includes(issued.token))
  const tools = await rpc(service, issued.token, 'tools/list')
  assert.ok(object(tools.result) && Array.isArray(tools.result.tools))
  assert.equal(tools.result.tools.length, 1)
  const denied = await rpc(service, issued.token, 'tools/call', {
    name: 'write_file',
    arguments: {},
  })
  assert.ok(object(denied.error))
  assert.equal(denied.error.code, -32001)
  assert.equal(calls(), 0)
  const accepted = await rpc(service, issued.token, 'tools/call', {
    name: 'read_file',
    arguments: { path: 'allowed' },
  })
  assert.ok(
    object(accepted.result) && object(accepted.result.structuredContent),
  )
  assert.equal(calls(), 1)
  await service.handle(
    request(`/api/v1/mcp/api-keys/${issued.id}/revoke`, 'POST', {}),
    context(),
  )
  assert.equal(
    (
      await service.handlePublic(
        request(
          '/mcp',
          'POST',
          { jsonrpc: '2.0', id: 1, method: 'tools/list' },
          issued.token,
        ),
      )
    )?.status,
    401,
  )
})

test('management is isolated by owner, tenant and deployment; unknown authorization fails closed', async () => {
  const { service, calls } = fixture({ authorizeTool: async () => false })
  await enable(service)
  const issued = await key(service)
  for (const other of [
    { ...actor, subjectId: 'other' },
    { ...actor, tenantId: 'other' },
  ]) {
    const listed = await service.handle(
      request('/api/v1/mcp/api-keys'),
      context(other),
    )
    assert.ok(listed)
    assert.deepEqual(await listed.json(), [])
    await assert.rejects(
      service.handle(
        request(`/api/v1/mcp/api-keys/${issued.id}/revoke`, 'POST', {}),
        context(other),
      ),
      (error: unknown) =>
        error instanceof KitFailure && error.code === 'not_found',
    )
  }
  await assert.rejects(
    service.handle(
      request('/api/v1/mcp/api-keys'),
      context({ ...actor, applicationId: 'other' }),
    ),
    (error: unknown) =>
      error instanceof KitFailure && error.code === 'forbidden',
  )
  const denied = await rpc(service, issued.token, 'tools/call', {
    name: 'read_file',
    arguments: { path: 'private' },
  })
  assert.ok(object(denied.error))
  assert.equal(denied.error.code, -32001)
  assert.equal(calls(), 0)
  const switched = fixture({
    applicationId: 'other',
    store: service.options.store,
  }).service
  assert.equal(
    (
      await switched.handlePublic(
        request(
          '/mcp',
          'POST',
          { jsonrpc: '2.0', id: 1, method: 'tools/list' },
          issued.token,
        ),
      )
    )?.status,
    401,
  )
})

test('enabled scopes are not granted scopes, and default server setting is disabled', async () => {
  const { service } = fixture()
  const issued = await key(service)
  assert.equal(
    (await service.handlePublic(request('/mcp', 'POST', {}, issued.token)))
      ?.status,
    401,
  )
  await enable(service)
  await service.handle(
    request('/api/v1/mcp/settings', 'PUT', {
      enabled: true,
      enabledScopes: ['mcp:access', 'files:write'],
    }),
    context(),
  )
  const tools = await rpc(service, issued.token, 'tools/list')
  assert.ok(object(tools.result))
  assert.deepEqual(tools.result.tools, [])
})

test('OAuth requires exact redirects, explicit consent, PKCE, one-time codes and rotating refresh tokens', async () => {
  const { service } = fixture()
  await enable(service)
  const registered = await value(
    await service.handlePublic(
      request('/oauth/register', 'POST', {
        client_name: 'External',
        redirect_uris: ['https://client.example/callback'],
      }),
    ),
  )
  assert.ok(typeof registered.client_id === 'string')
  const verifier = 'x'.repeat(43)
  const query = new URLSearchParams({
    client_id: registered.client_id,
    redirect_uri: 'https://client.example/callback',
    response_type: 'code',
    scope: 'mcp:access files:read',
    code_challenge_method: 'S256',
    code_challenge: await hash(verifier),
    state: 'state',
  })
  const path = `/oauth/authorize?${query}`
  const bad = new URLSearchParams(query)
  bad.set('redirect_uri', 'https://client.example/callback/other')
  await assert.rejects(
    service.handlePublic(request(`/oauth/authorize?${bad}`)),
    (error: unknown) =>
      error instanceof KitFailure && error.code === 'bad_request',
  )
  const consent = await service.handlePublic(request(path))
  assert.ok(consent)
  const csrf = /name="csrf" value="([^"]+)"/u.exec(await consent.text())?.[1]
  assert.ok(csrf)
  const approve = new Request(`${origin}${path}`, {
    method: 'POST',
    headers: {
      Origin: origin,
      'Content-Type': 'application/x-www-form-urlencoded',
    },
    body: new URLSearchParams({ csrf, decision: 'approve' }),
  })
  const redirect = await service.handlePublic(approve)
  assert.equal(redirect?.status, 302)
  const location = redirect?.headers.get('Location')
  assert.ok(location)
  const callback = new URL(location)
  assert.equal(callback.searchParams.get('state'), 'state')
  const code = callback.searchParams.get('code')
  assert.ok(code)
  const exchange = (form: Record<string, string>) =>
    service.handlePublic(
      new Request(`${origin}/oauth/token`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams(form),
      }),
    )
  const codeForm = {
    grant_type: 'authorization_code',
    client_id: registered.client_id,
    redirect_uri: 'https://client.example/callback',
    code,
    code_verifier: verifier,
  }
  assert.equal(
    (await exchange({ ...codeForm, code_verifier: 'y'.repeat(43) }))?.status,
    400,
  )
  const tokens = await value(await exchange(codeForm))
  assert.ok(
    typeof tokens.refresh_token === 'string' &&
      typeof tokens.access_token === 'string',
  )
  assert.equal((await exchange(codeForm))?.status, 400)
  const refreshForm = {
    grant_type: 'refresh_token',
    client_id: registered.client_id,
    refresh_token: tokens.refresh_token,
  }
  const rotated = await value(await exchange(refreshForm))
  assert.ok(typeof rotated.access_token === 'string')
  assert.equal((await exchange(refreshForm))?.status, 400)
  const grantsResponse = await service.handle(
    request('/api/v1/mcp/grants'),
    context(),
  )
  assert.ok(grantsResponse)
  const grants: unknown = await grantsResponse.json()
  assert.ok(Array.isArray(grants))
  assert.equal(grants.length, 1)
  const grant: unknown = grants[0]
  assert.ok(object(grant) && typeof grant.id === 'string')
  await service.handle(
    request(`/api/v1/mcp/grants/${grant.id}/revoke`, 'POST', {}),
    context(),
  )
  assert.equal(
    (
      await service.handlePublic(
        request(
          '/mcp',
          'POST',
          { jsonrpc: '2.0', id: 1, method: 'tools/list' },
          rotated.access_token,
        ),
      )
    )?.status,
    401,
  )
})

test('consent host rejection cannot mint code, and hostile Origin never dispatches a tool', async () => {
  const { service, calls } = fixture({ authorizeConsent: async () => false })
  await assert.rejects(
    service.handle(
      request('/api/v1/mcp/api-keys', 'POST', {
        name: 'client',
        scopes: ['mcp:access'],
      }),
      context(),
    ),
    (error: unknown) =>
      error instanceof KitFailure && error.code === 'forbidden',
  )
  const response = await service.handlePublic(
    new Request(`${origin}/mcp`, {
      method: 'POST',
      headers: { Origin: 'https://evil.example' },
      body: '{}',
    }),
  )
  assert.equal(response?.status, 403)
  assert.equal(calls(), 0)
})
