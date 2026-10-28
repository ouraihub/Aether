import assert from 'node:assert/strict'
import test from 'node:test'
import { KitFailure } from '../error.js'
import { isRecord } from '../guard.js'
import type { OutboundHttp } from '../ports.js'
import { MAX_MESSAGE_BYTES, McpRuntime, parseMcpCallResult } from './index.js'
import type { McpCachedTool, McpConfigStore, McpServerConfig } from './index.js'

const signal = () => new AbortController().signal
const cached = (overrides: Partial<McpCachedTool> = {}): McpCachedTool => ({
  name: 'echo',
  description: 'Echo',
  schema: { type: 'object' },
  readOnly: false,
  approval: 'ask',
  fetchedAt: Date.now(),
  ...overrides,
})
const server = (overrides: Partial<McpServerConfig> = {}): McpServerConfig => ({
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
class Store implements McpConfigStore {
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
  async replaceTools(id: string, tools: readonly McpCachedTool[]): Promise<void> {
    this.replacements++
    this.configs = this.configs.map((config) => (config.id === id ? { ...config, tools } : config))
  }
}
interface Sent {
  readonly url: string
  readonly init: RequestInit
  readonly rpc: Record<string, unknown>
}
class Wire implements OutboundHttp {
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
      return json(rpc.id, { protocolVersion: '2025-03-26' }, { 'Mcp-Session-Id': 'session-one' })
    if (rpc.method === 'notifications/initialized') return new Response(null, { status: 202 })
    return this.reply(sent)
  }
}
function json(id: unknown, result: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id, result }), {
    headers: { 'content-type': 'application/json', ...headers },
  })
}

for (const value of [
  null,
  {},
  { content: null },
  { content: [null] },
  { content: [{}] },
  { content: [{ type: '' }] },
  { content: [{ type: 'text' }] },
  { content: [], isError: 'yes' },
  { content: [], structuredContent: null },
]) {
  test(`invalid dispatched result stays unknown: ${JSON.stringify(value)}`, async () => {
    const wire = new Wire(({ rpc }) => json(rpc.id, value))
    const runtime = new McpRuntime(wire, new Store([server()]))
    const session = await runtime.session(signal())
    assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'invalid_response' })
    assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'tools/call').length, 1)
    await runtime.shutdown()
  })
}

test('preserves structured, nontext, extension and error content without interpretation', async () => {
  const result = {
    content: [
      { type: 'image', data: 'base64-data', mimeType: 'image/png' },
      { type: 'custom', nested: { key: ['value'] } },
      { type: 'text', text: 'error details' },
    ],
    structuredContent: { records: [{ ok: false }] },
    isError: true,
  }
  const wire = new Wire(({ rpc }) => json(rpc.id, result))
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', { message: 'hello' }, signal()), { kind: 'completed', ...result })
  assert.deepEqual(
    wire.sent.map((sent) => sent.rpc.method),
    ['initialize', 'notifications/initialized', 'tools/call'],
  )
  const call = wire.sent[2]
  assert.ok(call)
  const headers = new Headers(call.init.headers)
  assert.equal(headers.get('Mcp-Session-Id'), 'session-one')
  assert.equal(headers.get('MCP-Protocol-Version'), '2025-03-26')
  assert.equal(headers.get('Authorization'), 'Bearer private-value')
  assert.equal(headers.get('Accept'), 'application/json, text/event-stream')
  assert.equal(new URL(call.url).searchParams.get('key'), 'private-query')
  assert.equal(call.init.redirect, 'error')
  assert.deepEqual(call.rpc.arguments, undefined)
  assert.deepEqual(call.rpc.params, { name: 'echo', arguments: { message: 'hello' } })
  await runtime.shutdown()
})

test('empty content and structured object are valid; absent isError defaults false', () => {
  assert.deepEqual(parseMcpCallResult({ content: [], structuredContent: {} }), {
    kind: 'completed',
    content: [],
    structuredContent: {},
    isError: false,
  })
})

test('SSE matches JSON-RPC id rather than SSE event id and handles chunked multiline messages', async () => {
  const encoder = new TextEncoder()
  const wire = new Wire(({ rpc }) => {
    const payload = `id: ${String(rpc.id)}\ndata: {"jsonrpc":"2.0","id":999,"result":{}}\n\ndata: {"jsonrpc":"2.0",\r\ndata: "id":${String(rpc.id)},"result":{"content":[{"type":"text","text":"héllo"}]}}\r\n\r\n`
    const bytes = encoder.encode(payload)
    return new Response(
      new ReadableStream<Uint8Array>({
        start(controller) {
          for (let offset = 0; offset < bytes.length; offset += 7) controller.enqueue(bytes.slice(offset, offset + 7))
          controller.close()
        },
      }),
      { headers: { 'content-type': 'text/event-stream; charset=utf-8' } },
    )
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), {
    kind: 'completed',
    content: [{ type: 'text', text: 'héllo' }],
    isError: false,
  })
  await runtime.shutdown()
})

test('mismatched JSON response id is unknown and never replayed', async () => {
  const wire = new Wire(() => json(999, { content: [] }))
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'invalid_response' })
  assert.equal(wire.sent.length, 3)
  await runtime.shutdown()
})

test('valid RPC errors are definite failures but upstream messages are not exposed', async () => {
  const wire = new Wire(
    ({ rpc }) =>
      new Response(
        JSON.stringify({ jsonrpc: '2.0', id: rpc.id, error: { code: -32602, message: 'secret upstream payload' } }),
        { headers: { 'content-type': 'application/json' } },
      ),
  )
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), {
    kind: 'failed',
    code: '-32602',
    message: 'MCP server rejected the request',
  })
  await runtime.shutdown()
})

for (const type of ['application/json', 'text/event-stream']) {
  test(`total nontext plus structured response budget applies to ${type}`, async () => {
    const wire = new Wire(({ rpc }) => {
      const message = JSON.stringify({
        jsonrpc: '2.0',
        id: rpc.id,
        result: {
          content: [{ type: 'image', data: 'x'.repeat(MAX_MESSAGE_BYTES / 2) }],
          structuredContent: { data: 'y'.repeat(MAX_MESSAGE_BYTES / 2) },
        },
      })
      return new Response(type === 'text/event-stream' ? `data: ${message}\n\n` : message, {
        headers: { 'content-type': type },
      })
    })
    const runtime = new McpRuntime(wire, new Store([server()]))
    const session = await runtime.session(signal())
    assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'budget_exceeded' })
    assert.equal(wire.sent.length, 3)
    await runtime.shutdown()
  })
}

test('cancellation before dispatch is failed and sends no HTTP request', async () => {
  const wire = new Wire(() => {
    throw new Error('must not dispatch')
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const cancel = new AbortController()
  cancel.abort()
  assert.deepEqual(await session.call('mcp_one_echo', {}, cancel.signal), {
    kind: 'failed',
    code: 'cancelled',
    message: 'Cancelled',
  })
  assert.equal(wire.sent.length, 0)
  await runtime.shutdown()
})

test('cancel after dispatch is unknown even if injected transport ignores signal', async () => {
  const dispatched = Promise.withResolvers<void>()
  const response = Promise.withResolvers<Response>()
  const wire = new Wire(() => {
    dispatched.resolve()
    return response.promise
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const cancel = new AbortController()
  const call = session.call('mcp_one_echo', {}, cancel.signal)
  await dispatched.promise
  cancel.abort()
  assert.deepEqual(await call, { kind: 'unknown', reason: 'cancelled' })
  assert.equal(wire.sent.length, 3)
  response.resolve(json(3, { content: [] }))
  await runtime.shutdown()
})

test('network failure after dispatch is unknown, with no automatic replay', async () => {
  const wire = new Wire(() => {
    throw new Error('network secret')
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'network' })
  assert.equal(wire.sent.length, 3)
  await runtime.shutdown()
})

test('enabled and trusted catalogue and approval/config snapshots remain fixed', async () => {
  const store = new Store([
    server(),
    server({ id: 'disabled', slug: 'disabled', enabled: false }),
    server({ id: 'untrusted', slug: 'untrusted', trusted: false }),
  ])
  const wire = new Wire(({ rpc }) => json(rpc.id, { content: [] }))
  const runtime = new McpRuntime(wire, store)
  const session = await runtime.session(signal())
  assert.equal(wire.sent.length, 0)
  assert.deepEqual(
    session.tools().map((tool) => [tool.prefixed, tool.approval]),
    [['mcp_one_echo', 'ask']],
  )
  store.configs = [server({ name: 'changed', slug: 'changed', trusted: false, tools: [cached({ approval: 'deny' })] })]
  assert.equal(session.tools()[0]?.server, 'Server one')
  assert.equal(session.tools()[0]?.approval, 'ask')
  assert.equal((await session.call('mcp_one_echo', {}, signal())).kind, 'completed')
  await runtime.shutdown()
})

test('discovery paginates, bounds descriptions/schema, retains approval, and defaults readOnly tools to ask', async () => {
  let pages = 0
  const wire = new Wire(({ rpc }) => {
    pages++
    if (pages === 1)
      return json(rpc.id, {
        tools: [{ name: 'echo', description: 'd'.repeat(1100), inputSchema: {}, annotations: { readOnlyHint: true } }],
        nextCursor: 'next',
      })
    assert.deepEqual(rpc.params, { cursor: 'next' })
    return json(rpc.id, {
      tools: [
        { name: 'new', inputSchema: {}, annotations: { readOnlyHint: true } },
        { name: 'huge', inputSchema: { description: 'x'.repeat(33 * 1024) } },
      ],
    })
  })
  const store = new Store([server({ enabled: false, trusted: false, tools: [cached({ approval: 'deny' })] })])
  const runtime = new McpRuntime(wire, store)
  const tools = await runtime.refresh('one', signal())
  assert.deepEqual(
    tools.map((tool) => [tool.name, tool.approval, tool.readOnly]),
    [
      ['echo', 'deny', true],
      ['new', 'ask', true],
    ],
  )
  assert.equal(tools[0]?.description.length, 1024)
  assert.equal(store.replacements, 1)
  await runtime.shutdown()
})

test('uncached discovery connection is reused for tool call and isolated discovery failure excludes only that server', async () => {
  const wire = new Wire(({ rpc, url }) => {
    if (new URL(url).pathname === '/bad') throw new Error('unreachable')
    return json(rpc.id, rpc.method === 'tools/list' ? { tools: [{ name: 'echo', inputSchema: {} }] } : { content: [] })
  })
  const store = new Store([
    server({ id: 'bad', slug: 'bad', url: 'https://mcp.example/bad', tools: [] }),
    server({ tools: [] }),
  ])
  const runtime = new McpRuntime(wire, store)
  const session = await runtime.session(signal())
  assert.equal(session.tools().length, 1)
  const before = wire.sent.length
  assert.equal((await session.call('mcp_one_echo', {}, signal())).kind, 'completed')
  assert.equal(wire.sent.length, before + 1)
  await runtime.shutdown()
})

test('failed explicit refresh leaves previous cache intact', async () => {
  const wire = new Wire(({ rpc }) => json(rpc.id, { tools: 'invalid' }))
  const store = new Store([server()])
  const runtime = new McpRuntime(wire, store)
  await assert.rejects(runtime.refresh('one', signal()), { code: 'upstream' })
  assert.equal(store.replacements, 0)
  assert.equal(store.configs[0]?.tools[0]?.name, 'echo')
  await runtime.shutdown()
})

test('discovery can reinitialize once after session 404', async () => {
  let lists = 0
  const wire = new Wire(({ rpc }) =>
    ++lists === 1 ? new Response(null, { status: 404 }) : json(rpc.id, { tools: [{ name: 'echo', inputSchema: {} }] }),
  )
  const runtime = new McpRuntime(wire, new Store([server()]))
  assert.equal((await runtime.refresh('one', signal())).length, 1)
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'initialize').length, 2)
  assert.equal(lists, 2)
  await runtime.shutdown()
})

test('side effect session 404 is unknown and not replayed; only a later explicit call reconnects', async () => {
  let calls = 0
  const wire = new Wire(({ rpc }) =>
    ++calls === 1 ? new Response(null, { status: 404 }) : json(rpc.id, { content: [] }),
  )
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'network' })
  assert.equal(calls, 1)
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'initialize').length, 1)
  assert.equal((await session.call('mcp_one_echo', {}, signal())).kind, 'completed')
  assert.equal(calls, 2)
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'initialize').length, 2)
  await runtime.shutdown()
})

test('stale cache is immediate, refresh is deduplicated, and concurrent shutdown drains once', async () => {
  const dispatched = Promise.withResolvers<void>()
  const response = Promise.withResolvers<Response>()
  const wire = new Wire(() => {
    dispatched.resolve()
    return response.promise
  })
  const store = new Store([server({ tools: [cached({ fetchedAt: 0 })] })])
  const runtime = new McpRuntime(wire, store)
  const session = await runtime.session(signal())
  assert.equal(session.tools().length, 1)
  await dispatched.promise
  assert.equal((await runtime.session(signal())).tools().length, 1)
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'tools/list').length, 1)
  const first = runtime.shutdown()
  const second = runtime.shutdown()
  assert.equal(first, second)
  await Promise.all([first, second])
  assert.equal(store.replacements, 0)
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), {
    kind: 'failed',
    code: 'cancelled',
    message: 'Cancelled',
  })
  assert.equal((await runtime.session(signal())).tools().length, 0)
  await assert.rejects(runtime.refresh('one', signal()), { code: 'cancelled' })
  response.resolve(json(3, { tools: [] }))
})

test('runtime shutdown makes an already dispatched tool call unknown', async () => {
  const dispatched = Promise.withResolvers<void>()
  const response = Promise.withResolvers<Response>()
  const wire = new Wire(() => {
    dispatched.resolve()
    return response.promise
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const call = session.call('mcp_one_echo', {}, signal())
  await dispatched.promise
  await runtime.shutdown()
  assert.deepEqual(await call, { kind: 'unknown', reason: 'cancelled' })
  response.resolve(json(3, { content: [] }))
})

test('unsupported initialization and rejected URL fail before tools/call dispatch', async () => {
  const wire: OutboundHttp = {
    async fetch(_url, init) {
      const rpc: unknown = JSON.parse(String(init.body))
      assert.ok(typeof rpc === 'object' && rpc !== null && 'id' in rpc)
      return json(rpc.id, { protocolVersion: 'unknown' })
    },
  }
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), {
    kind: 'failed',
    code: 'unsupported',
    message: 'Unsupported MCP protocol version',
  })
  await runtime.shutdown()
  const invalidWire = new Wire(() => {
    throw new Error('must not send')
  })
  const invalidRuntime = new McpRuntime(invalidWire, new Store([server({ url: 'https://user:password@mcp.example' })]))
  const invalidSession = await invalidRuntime.session(signal())
  const result = await invalidSession.call('mcp_one_echo', {}, signal())
  assert.equal(result.kind, 'failed')
  assert.equal(invalidWire.sent.length, 0)
  await invalidRuntime.shutdown()
})

test('request timer failure remains unknown after dispatch', async () => {
  const wire = new Wire(() => {
    throw new KitFailure('timeout', 'Request timed out')
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'timeout' })
  await runtime.shutdown()
})

test('discovery rejects an unbounded page chain without replacing successful cache', async () => {
  let page = 0
  const wire = new Wire(({ rpc }) => json(rpc.id, { tools: [], nextCursor: `page-${++page}` }))
  const store = new Store([server()])
  const runtime = new McpRuntime(wire, store)
  await assert.rejects(runtime.refresh('one', signal()), { code: 'upstream', details: { reason: 'budget_exceeded' } })
  assert.equal(page, 10)
  assert.equal(store.replacements, 0)
  await runtime.shutdown()
})

test('discovery rejects more than 200 tools and repeated cursors', async () => {
  for (const tooMany of [true, false]) {
    const wire = new Wire(({ rpc }) =>
      json(
        rpc.id,
        tooMany
          ? { tools: Array.from({ length: 201 }, (_, index) => ({ name: `tool-${index}`, inputSchema: {} })) }
          : { tools: [], nextCursor: 'same' },
      ),
    )
    const store = new Store([server()])
    const runtime = new McpRuntime(wire, store)
    await assert.rejects(runtime.refresh('one', signal()))
    assert.equal(store.replacements, 0)
    await runtime.shutdown()
  }
})

test('exactly 2 MiB JSON reply is accepted and counts UTF-8 encoded bytes', async () => {
  const wire = new Wire(({ rpc }) => {
    const empty = JSON.stringify({ jsonrpc: '2.0', id: rpc.id, result: { content: [{ type: 'image', data: '' }] } })
    const data = 'x'.repeat(MAX_MESSAGE_BYTES - new TextEncoder().encode(empty).byteLength)
    return json(rpc.id, { content: [{ type: 'image', data }] })
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const result = await session.call('mcp_one_echo', {}, signal())
  assert.equal(result.kind, 'completed')
  await runtime.shutdown()
})

test('malformed UTF-8 response is invalid rather than a definite failure', async () => {
  const wire = new Wire(() => new Response(new Uint8Array([0xff]), { headers: { 'content-type': 'application/json' } }))
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), { kind: 'unknown', reason: 'invalid_response' })
  await runtime.shutdown()
})

test('shutdown awaits a cache transaction already started by refresh', async () => {
  const committing = Promise.withResolvers<void>()
  const committed = Promise.withResolvers<void>()
  const store: McpConfigStore = {
    async list() {
      return [server()]
    },
    async get() {
      return server()
    },
    async replaceTools() {
      committing.resolve()
      await committed.promise
    },
  }
  const wire = new Wire(({ rpc }) => json(rpc.id, { tools: [] }))
  const runtime = new McpRuntime(wire, store)
  const refresh = runtime.refresh('one', signal())
  const rejection = assert.rejects(refresh, { code: 'cancelled' })
  await committing.promise
  let drained = false
  const shutdown = runtime.shutdown().then(() => {
    drained = true
  })
  await Promise.resolve()
  assert.equal(drained, false)
  committed.resolve()
  await Promise.all([shutdown, rejection])
  assert.equal(drained, true)
})

test('stale refreshes are queued with bounded concurrency and all canceled on shutdown', async () => {
  const dispatched = Promise.withResolvers<void>()
  const response = Promise.withResolvers<Response>()
  const wire = new Wire(() => {
    dispatched.resolve()
    return response.promise
  })
  const store = new Store(
    Array.from({ length: 8 }, (_, index) =>
      server({ id: String(index), slug: `s${index}`, tools: [cached({ fetchedAt: 0 })] }),
    ),
  )
  const runtime = new McpRuntime(wire, store)
  assert.equal((await runtime.session(signal())).tools().length, 8)
  await dispatched.promise
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'initialize').length, 1)
  await runtime.shutdown()
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'initialize').length, 1)
  response.resolve(json(3, { tools: [] }))
})

test('negotiates the older supported protocol and initialized notification has no id', async () => {
  let dispatchedCalls = 0
  const wire: OutboundHttp = {
    async fetch(_url, init) {
      const rpc: unknown = JSON.parse(String(init.body))
      assert.ok(isRecord(rpc))
      if (rpc.method === 'initialize') {
        assert.ok(isRecord(rpc.params))
        assert.equal(rpc.params.protocolVersion, '2025-03-26')
        return json(rpc.id, { protocolVersion: '2024-11-05' })
      }
      assert.equal(new Headers(init.headers).get('MCP-Protocol-Version'), '2024-11-05')
      if (rpc.method === 'notifications/initialized') {
        assert.equal('id' in rpc, false)
        return new Response(null, { status: 202 })
      }
      dispatchedCalls++
      return json(rpc.id, { content: [] })
    },
  }
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.equal((await session.call('mcp_one_echo', {}, signal())).kind, 'completed')
  assert.equal(dispatchedCalls, 1)
  await runtime.shutdown()
})

test('oversized arguments are a definite failure and never dispatch tools/call', async () => {
  const wire = new Wire(() => {
    throw new Error('must not dispatch tools/call')
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const result = await session.call('mcp_one_echo', { text: 'é'.repeat(MAX_MESSAGE_BYTES / 2) }, signal())
  assert.equal(result.kind, 'failed')
  if (result.kind === 'failed') assert.equal(result.code, 'bad_request')
  assert.equal(wire.sent.filter((sent) => sent.rpc.method === 'tools/call').length, 0)
  await runtime.shutdown()
})

test('direct core configs require explicit private-address consent before network dispatch', async () => {
  for (const url of [
    'https://localhost/rpc',
    'https://127.0.0.1/rpc',
    'https://127.1/rpc',
    'https://10.0.0.1/rpc',
    'https://192.168.1.1/rpc',
    'https://[::1]/rpc',
    'https://[::ffff:127.0.0.1]/rpc',
    'https://[fd00::1]/rpc',
    'https://169.254.169.254/rpc',
    'http://mcp.example/rpc',
  ]) {
    for (const allowPrivate of [false, true]) {
      const wire = new Wire(({ rpc }) => json(rpc.id, { content: [] }))
      const runtime = new McpRuntime(wire, new Store([server({ url, allowPrivate })]))
      const session = await runtime.session(signal())
      const result = await session.call('mcp_one_echo', {}, signal())
      assert.equal(result.kind, allowPrivate ? 'completed' : 'failed', `${url}: allowPrivate=${String(allowPrivate)}`)
      assert.equal(wire.sent.length, allowPrivate ? 3 : 0)
      await runtime.shutdown()
    }
  }
})
