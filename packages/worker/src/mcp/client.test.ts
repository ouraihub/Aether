import assert from 'node:assert/strict'
import test from 'node:test'
import { KitFailure } from '@ouraihub/aether-core/error'
import { isRecord } from '@ouraihub/aether-core/guard'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import { ownerKey, type Actor, type WorkerContext } from '../context.js'
import { createCredentialPort, type CredentialPort } from '../credentials.js'
import type { DocumentStore, StoredDocument } from '../storage.js'
import { McpClientSettings } from './client.js'

const actor: Actor = {
  subjectId: 'owner',
  applicationId: 'app',
  tenantId: 'tenant',
  kind: 'user',
  sessionId: 'session',
  scopes: [],
}
const endpoint = '/api/v1/mcp/servers'

class TestDocuments implements DocumentStore {
  readonly rows = new Map<string, StoredDocument>()
  private address(owner: Actor, collection: string, key: string): string {
    return JSON.stringify([ownerKey(owner), collection, key])
  }
  async get(
    owner: Actor,
    collection: string,
    key: string,
  ): Promise<StoredDocument | null> {
    return structuredClone(
      this.rows.get(this.address(owner, collection, key)) ?? null,
    )
  }
  async list(
    owner: Actor,
    collection: string,
  ): Promise<readonly StoredDocument[]> {
    const prefix =
      JSON.stringify([ownerKey(owner), collection]).slice(0, -1) + ','
    return [...this.rows]
      .filter(([key]) => key.startsWith(prefix))
      .map(([, value]) => structuredClone(value))
  }
  async compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expected: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    const address = this.address(owner, collection, key)
    const existing = this.rows.get(address)
    if ((existing?.revision ?? null) !== expected)
      throw new KitFailure('conflict', 'Document revision conflict')
    const document = {
      key,
      revision: (existing?.revision ?? 0) + 1,
      value: structuredClone(value),
    }
    this.rows.set(address, document)
    return structuredClone(document)
  }
  async remove(
    owner: Actor,
    collection: string,
    key: string,
    expected: number,
  ): Promise<void> {
    const address = this.address(owner, collection, key)
    if (this.rows.get(address)?.revision !== expected)
      throw new KitFailure('conflict', 'Document revision conflict')
    this.rows.delete(address)
  }
}
function record(value: unknown): Record<string, unknown> {
  assert.ok(isRecord(value))
  return value
}
function serverRecords(value: unknown): Record<string, unknown>[] {
  assert.ok(Array.isArray(value))
  return value.map(record)
}
function context(owner: Actor = actor): WorkerContext {
  return {
    actor: owner,
    signal: new AbortController().signal,
    requestId: 'request',
    now: Date.now(),
  }
}
async function call(
  settings: McpClientSettings,
  method: string,
  path = endpoint,
  input?: unknown,
  owner: Actor = actor,
): Promise<unknown> {
  const response = await settings.handle(
    new Request(`https://worker.example${path}`, {
      method,
      ...(input === undefined
        ? {}
        : {
            body: JSON.stringify(input),
            headers: { 'Content-Type': 'application/json' },
          }),
    }),
    context(owner),
  )
  assert.ok(response)
  return response.json()
}
async function setup() {
  const key = await crypto.subtle.generateKey(
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
  const credentials = createCredentialPort({
    currentKeyId: 'test',
    keys: new Map([['test', key]]),
  })
  const store = new TestDocuments()
  const sent: { url: string; headers: Headers }[] = []
  const remote = {
    tools: [
      {
        name: 'read',
        description: 'Read',
        inputSchema: { type: 'object' },
        annotations: { readOnlyHint: true },
      },
    ],
    fail: false,
    beforeList: async (): Promise<void> => {},
  }
  const outbound: OutboundHttp = {
    async fetch(url, init) {
      sent.push({ url, headers: new Headers(init.headers) })
      assert.equal(typeof init.body, 'string')
      const message = record(JSON.parse(String(init.body)))
      if (message.method === 'notifications/initialized')
        return new Response(null, { status: 202 })
      if (message.method === 'initialize')
        return Response.json({
          jsonrpc: '2.0',
          id: message.id,
          result: {
            protocolVersion: '2025-03-26',
            capabilities: {},
            serverInfo: { name: 'test', version: '1' },
          },
        })
      assert.equal(message.method, 'tools/list')
      await remote.beforeList()
      if (remote.fail) return new Response('unavailable', { status: 503 })
      return Response.json({
        jsonrpc: '2.0',
        id: message.id,
        result: { tools: remote.tools },
      })
    },
  }
  return {
    store,
    credentials,
    sent,
    remote,
    outbound,
    settings: new McpClientSettings(store, credentials, outbound),
  }
}
function saveInput(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    slug: 'remote',
    name: 'Remote',
    transport: { kind: 'http', url: 'https://mcp.example/mcp?page=2' },
    ...overrides,
  }
}
async function create(
  settings: McpClientSettings,
  input = saveInput(),
  owner: Actor = actor,
): Promise<Record<string, unknown>> {
  const servers = serverRecords(
    await call(settings, 'POST', endpoint, input, owner),
  )
  const server = servers.at(-1)
  assert.ok(server)
  return server
}
function idOf(server: Record<string, unknown>): string {
  assert.equal(typeof server.id, 'string')
  return String(server.id)
}

test('defaults are disabled/untrusted, HTTP secrets are encrypted and only their names are returned', async () => {
  const fixture = await setup()
  const server = await create(
    fixture.settings,
    saveInput({
      secret: {
        headers: { Authorization: 'Bearer sensitive-header' },
        query: { token: 'sensitive-query' },
      },
    }),
  )
  assert.equal(server.enabled, false)
  assert.equal(server.trusted, false)
  assert.equal(server.allowPrivate, false)
  assert.deepEqual(server.secretKeys, ['Authorization', 'token'])
  const stored = JSON.stringify([...fixture.store.rows.values()])
  assert.ok(
    !stored.includes('sensitive-header') && !stored.includes('sensitive-query'),
  )
  assert.ok(!JSON.stringify(server).includes('sensitive-'))
  await call(fixture.settings, 'POST', `${endpoint}/${idOf(server)}/refresh`)
  assert.equal(
    fixture.sent[0]?.headers.get('Authorization'),
    'Bearer sensitive-header',
  )
  assert.equal(
    new URL(fixture.sent[0]?.url ?? '').searchParams.get('token'),
    'sensitive-query',
  )
  assert.equal(record(server.transport).url, 'https://mcp.example/mcp?page=2')
})

test('only enabled and trusted settings enter actor runtime sessions', async () => {
  const fixture = await setup()
  const server = await create(
    fixture.settings,
    saveInput({ enabled: true, trusted: false }),
  )
  const runtime = fixture.settings.runtime(actor)
  assert.deepEqual(
    (await runtime.session(new AbortController().signal)).tools(),
    [],
  )
  assert.equal(fixture.sent.length, 0)
  await call(
    fixture.settings,
    'PUT',
    `${endpoint}/${idOf(server)}`,
    saveInput({ enabled: true, trusted: true }),
  )
  const session = await runtime.session(new AbortController().signal)
  assert.deepEqual(
    session.tools().map((tool) => [tool.prefixed, tool.approval]),
    [['mcp_remote_read', 'ask']],
  )
  await call(
    fixture.settings,
    'PUT',
    `${endpoint}/${idOf(server)}`,
    saveInput({ enabled: false, trusted: false }),
  )
  assert.deepEqual(
    (await runtime.session(new AbortController().signal)).tools(),
    [],
  )
  assert.deepEqual(
    session.tools().map((tool) => tool.prefixed),
    ['mcp_remote_read'],
  )
  await runtime.shutdown()
})

test('owner, application and tenant isolate configs, mutations and runtime credentials', async () => {
  const fixture = await setup()
  const server = await create(
    fixture.settings,
    saveInput({
      secret: { headers: { Authorization: 'private-owner-value' } },
    }),
  )
  const id = idOf(server)
  for (const other of [
    { ...actor, subjectId: 'other' },
    { ...actor, applicationId: 'other-app' },
    { ...actor, tenantId: 'other-tenant' },
    { ...actor, tenantId: null },
  ]) {
    assert.deepEqual(
      await call(fixture.settings, 'GET', endpoint, undefined, other),
      [],
    )
    for (const [method, path, input] of [
      ['PUT', `${endpoint}/${id}`, saveInput()],
      ['DELETE', `${endpoint}/${id}`, undefined],
      ['POST', `${endpoint}/${id}/refresh`, undefined],
      ['PUT', `${endpoint}/${id}/tools/read/approval`, { approval: 'auto' }],
    ] as const)
      await assert.rejects(call(fixture.settings, method, path, input, other), {
        code: 'not_found',
      })
    const own = await create(
      fixture.settings,
      saveInput({
        secret: {
          headers: {
            Authorization: `value-${other.applicationId}-${other.tenantId}-${other.subjectId}`,
          },
        },
      }),
      other,
    )
    await call(
      fixture.settings,
      'POST',
      `${endpoint}/${idOf(own)}/refresh`,
      undefined,
      other,
    )
    assert.equal(
      fixture.sent.at(-1)?.headers.get('Authorization'),
      `value-${other.applicationId}-${other.tenantId}-${other.subjectId}`,
    )
    assert.notEqual(
      fixture.settings.runtime(other),
      fixture.settings.runtime(actor),
    )
  }
  assert.equal(
    fixture.settings.runtime({ ...actor, sessionId: 'second-session' }),
    fixture.settings.runtime(actor),
  )
  assert.equal(serverRecords(await call(fixture.settings, 'GET')).length, 1)
})

test('secrets keep, replace and clear without leaking old values; seal failure leaves config unchanged', async () => {
  const fixture = await setup()
  const server = await create(
    fixture.settings,
    saveInput({ secret: { headers: { Authorization: 'first-secret' } } }),
  )
  const path = `${endpoint}/${idOf(server)}`
  await call(fixture.settings, 'PUT', path, saveInput({ name: 'Renamed' }))
  await call(fixture.settings, 'POST', `${path}/refresh`)
  assert.equal(
    fixture.sent.at(-1)?.headers.get('Authorization'),
    'first-secret',
  )
  await call(
    fixture.settings,
    'PUT',
    path,
    saveInput({ secret: { query: { key: 'replacement-secret' } } }),
  )
  await call(fixture.settings, 'POST', `${path}/refresh`)
  assert.equal(fixture.sent.at(-1)?.headers.get('Authorization'), null)
  assert.equal(
    new URL(fixture.sent.at(-1)?.url ?? '').searchParams.get('key'),
    'replacement-secret',
  )
  const before = JSON.stringify([...fixture.store.rows])
  const broken: CredentialPort = {
    ...fixture.credentials,
    async seal() {
      throw new KitFailure('unsupported', 'No encryption key')
    },
  }
  const settings = new McpClientSettings(
    fixture.store,
    broken,
    fixture.outbound,
  )
  await assert.rejects(
    call(
      settings,
      'PUT',
      path,
      saveInput({ secret: { headers: { Authorization: 'never-persisted' } } }),
    ),
    { code: 'unsupported' },
  )
  assert.equal(JSON.stringify([...fixture.store.rows]), before)
  await call(fixture.settings, 'PUT', path, saveInput({ secret: {} }))
  await call(fixture.settings, 'POST', `${path}/refresh`)
  assert.equal(fixture.sent.at(-1)?.headers.get('Authorization'), null)
  assert.equal(
    new URL(fixture.sent.at(-1)?.url ?? '').searchParams.get('key'),
    null,
  )
  assert.deepEqual(
    serverRecords(await call(fixture.settings, 'GET'))[0]?.secretKeys,
    [],
  )
})

test('URL credentials, stdio, invalid headers, duplicate slugs and malformed flags are rejected', async () => {
  const { settings } = await setup()
  for (const url of [
    'https://user:password@mcp.example/mcp',
    'https://mcp.example/?API_KEY=abc',
    'https://mcp.example/?q=sk-abcdefghijklmnopqrstuvwxyz',
    'https://mcp.example/?q=Bearer%20abcdefghijklmnopqrst',
    'file:///tmp/mcp',
    'http://mcp.example/',
  ]) {
    await assert.rejects(
      create(settings, saveInput({ transport: { kind: 'http', url } })),
      { code: 'bad_request' },
    )
  }
  await assert.rejects(
    create(
      settings,
      saveInput({ transport: { kind: 'stdio', command: 'mcp', args: [] } }),
    ),
    {
      code: 'unsupported',
    },
  )
  await assert.rejects(
    create(settings, saveInput({ secret: { env: { TOKEN: 'value' } } })),
    { code: 'bad_request' },
  )
  await assert.rejects(
    create(
      settings,
      saveInput({
        secret: { headers: { Authorization: 'bad\r\nInjected: yes' } },
      }),
    ),
    { code: 'bad_request' },
  )
  for (const slug of ['UPPER', 'hyphen-slug', '', 'a'.repeat(25)])
    await assert.rejects(create(settings, saveInput({ slug })), {
      code: 'bad_request',
    })
  await assert.rejects(create(settings, saveInput({ enabled: 'yes' })), {
    code: 'bad_request',
  })
  await create(settings)
  await assert.rejects(create(settings), {
    code: 'conflict',
    details: { reason: 'duplicate_slug', field: 'slug' },
  })
})

test('private and loopback targets require explicit allowPrivate for HTTP and HTTPS', async () => {
  const { settings } = await setup()
  for (const [index, url] of [
    'http://localhost/mcp',
    'https://localhost/mcp',
    'https://127.0.0.1/mcp',
    'https://[::1]/mcp',
    'https://10.0.0.1/mcp',
    'https://192.168.1.1/mcp',
    'https://[fd00::1]/mcp',
  ].entries()) {
    await assert.rejects(
      create(settings, saveInput({ transport: { kind: 'http', url } })),
      { code: 'bad_request' },
    )
    const server = await create(
      settings,
      saveInput({
        slug: `private_${index}`,
        transport: { kind: 'http', url },
        allowPrivate: true,
      }),
    )
    assert.equal(server.allowPrivate, true)
  }
})

test('refresh preserves successful cache and approvals, assigns new tools ask and removes vanished tools', async () => {
  const fixture = await setup()
  const server = await create(fixture.settings)
  const path = `${endpoint}/${idOf(server)}`
  assert.deepEqual(await call(fixture.settings, 'POST', `${path}/refresh`), [
    { name: 'read', description: 'Read', readOnly: true, approval: 'ask' },
  ])
  await call(fixture.settings, 'PUT', `${path}/tools/read/approval`, {
    approval: 'deny',
  })
  fixture.remote.tools.push({
    name: 'write',
    description: 'Write',
    inputSchema: { type: 'object' },
    annotations: { readOnlyHint: false },
  })
  const refreshed = serverRecords(
    await call(fixture.settings, 'POST', `${path}/refresh`),
  )
  assert.deepEqual(
    refreshed.map((entry) => [entry.name, entry.approval]),
    [
      ['read', 'deny'],
      ['write', 'ask'],
    ],
  )
  fixture.remote.fail = true
  await assert.rejects(call(fixture.settings, 'POST', `${path}/refresh`))
  assert.deepEqual(
    serverRecords(await call(fixture.settings, 'GET'))[0]?.tools,
    refreshed,
  )
  fixture.remote.fail = false
  fixture.remote.tools = []
  assert.deepEqual(await call(fixture.settings, 'POST', `${path}/refresh`), [])
  await assert.rejects(
    call(fixture.settings, 'PUT', `${path}/tools/read/approval`, {
      approval: 'auto',
    }),
    {
      code: 'not_found',
    },
  )
})

test('delete tombstone prevents delayed refresh resurrection', async () => {
  const fixture = await setup()
  const server = await create(fixture.settings)
  const path = `${endpoint}/${idOf(server)}`
  const arrived = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  fixture.remote.beforeList = async () => {
    arrived.resolve()
    await release.promise
  }
  const refresh = call(fixture.settings, 'POST', `${path}/refresh`)
  await arrived.promise
  assert.deepEqual(await call(fixture.settings, 'DELETE', path), [])
  release.resolve()
  await assert.rejects(refresh, { code: 'not_found' })
  await assert.rejects(call(fixture.settings, 'PUT', path, saveInput()), {
    code: 'not_found',
  })
  assert.deepEqual(await call(fixture.settings, 'GET'), [])
  assert.ok(
    JSON.stringify([...fixture.store.rows.values()]).includes(idOf(server)),
  )
})

test('CAS prevents concurrent save overwriting another successful mutation', async () => {
  const fixture = await setup()
  const server = await create(fixture.settings)
  const path = `${endpoint}/${idOf(server)}`
  const arrived = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  const delayed: CredentialPort = {
    ...fixture.credentials,
    async seal(scope, value) {
      arrived.resolve()
      await release.promise
      return fixture.credentials.seal(scope, value)
    },
  }
  const other = new McpClientSettings(fixture.store, delayed, fixture.outbound)
  const saving = call(
    other,
    'PUT',
    path,
    saveInput({ name: 'Stale', secret: { query: { token: 'stale-secret' } } }),
  )
  await arrived.promise
  await call(fixture.settings, 'PUT', path, saveInput({ name: 'Winner' }))
  release.resolve()
  await assert.rejects(saving, { code: 'conflict' })
  assert.equal(
    serverRecords(await call(fixture.settings, 'GET'))[0]?.name,
    'Winner',
  )
  assert.ok(
    !JSON.stringify([...fixture.store.rows.values()]).includes('stale-secret'),
  )
})

test('unreadable ciphertext and transplanted record credentials fail closed without secret errors', async () => {
  const fixture = await setup()
  const first = await create(
    fixture.settings,
    saveInput({
      secret: { headers: { Authorization: 'bound-to-first-record' } },
    }),
  )
  const second = await create(
    fixture.settings,
    saveInput({
      slug: 'second',
      secret: { headers: { Authorization: 'second-record' } },
    }),
  )
  const row = await fixture.store.get(actor, 'mcp-client', 'servers')
  assert.ok(row)
  const state = record(row.value)
  const servers = serverRecords(state.servers)
  const a = servers.find((server) => server.id === first.id)
  const b = servers.find((server) => server.id === second.id)
  assert.ok(a && b)
  b.secret = a.secret
  await fixture.store.compareAndSet(
    actor,
    'mcp-client',
    'servers',
    row.revision,
    state,
  )
  await assert.rejects(
    call(fixture.settings, 'POST', `${endpoint}/${idOf(second)}/refresh`),
    {
      code: 'needs_config',
      details: { reason: 'key_unreadable' },
    },
  )
  assert.equal(fixture.sent.length, 0)
  assert.ok(
    !JSON.stringify(await call(fixture.settings, 'GET')).includes(
      'bound-to-first-record',
    ),
  )
  for (const other of [
    { ...actor, subjectId: 'other' },
    { ...actor, applicationId: 'other-app' },
    { ...actor, tenantId: null },
  ]) {
    await fixture.store.compareAndSet(
      other,
      'mcp-client',
      'servers',
      null,
      state,
    )
    await assert.rejects(
      call(
        fixture.settings,
        'POST',
        `${endpoint}/${idOf(first)}/refresh`,
        undefined,
        other,
      ),
      {
        code: 'needs_config',
      },
    )
  }
})

test('stored shapes are strict and unrelated routes are not claimed', async () => {
  const fixture = await setup()
  await fixture.store.compareAndSet(actor, 'mcp-client', 'servers', null, {
    version: 1,
    servers: [{ id: 'broken' }],
    deleted: [],
  })
  await assert.rejects(call(fixture.settings, 'GET'), { code: 'internal' })
  assert.equal(
    await fixture.settings.handle(
      new Request('https://worker.example/api/v1/mcp/server'),
      context(),
    ),
    null,
  )
})
