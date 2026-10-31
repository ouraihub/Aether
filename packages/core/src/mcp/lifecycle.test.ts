import assert from 'node:assert/strict'
import test from 'node:test'
import { KitFailure } from '../error.js'
import { isRecord } from '../guard.js'
import type { OutboundHttp } from '../ports.js'
import { MAX_MESSAGE_BYTES, McpRuntime } from './index.js'
import type { McpConfigStore } from './index.js'
import { signal, cached, server, Store, Wire, json } from './test-support.js'
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
  assert.equal(
    wire.sent.filter((sent) => sent.rpc.method === 'tools/list').length,
    1,
  )
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
  const invalidRuntime = new McpRuntime(
    invalidWire,
    new Store([server({ url: 'https://user:password@mcp.example' })]),
  )
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
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), {
    kind: 'unknown',
    reason: 'timeout',
  })
  await runtime.shutdown()
})

test('discovery rejects an unbounded page chain without replacing successful cache', async () => {
  let page = 0
  const wire = new Wire(({ rpc }) =>
    json(rpc.id, { tools: [], nextCursor: `page-${++page}` }),
  )
  const store = new Store([server()])
  const runtime = new McpRuntime(wire, store)
  await assert.rejects(runtime.refresh('one', signal()), {
    code: 'upstream',
    details: { reason: 'budget_exceeded' },
  })
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
          ? {
              tools: Array.from({ length: 201 }, (_, index) => ({
                name: `tool-${index}`,
                inputSchema: {},
              })),
            }
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
    const empty = JSON.stringify({
      jsonrpc: '2.0',
      id: rpc.id,
      result: { content: [{ type: 'image', data: '' }] },
    })
    const data = 'x'.repeat(
      MAX_MESSAGE_BYTES - new TextEncoder().encode(empty).byteLength,
    )
    return json(rpc.id, { content: [{ type: 'image', data }] })
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const result = await session.call('mcp_one_echo', {}, signal())
  assert.equal(result.kind, 'completed')
  await runtime.shutdown()
})

test('malformed UTF-8 response is invalid rather than a definite failure', async () => {
  const wire = new Wire(
    () =>
      new Response(new Uint8Array([0xff]), {
        headers: { 'content-type': 'application/json' },
      }),
  )
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  assert.deepEqual(await session.call('mcp_one_echo', {}, signal()), {
    kind: 'unknown',
    reason: 'invalid_response',
  })
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
      server({
        id: String(index),
        slug: `s${index}`,
        tools: [cached({ fetchedAt: 0 })],
      }),
    ),
  )
  const runtime = new McpRuntime(wire, store)
  assert.equal((await runtime.session(signal())).tools().length, 8)
  await dispatched.promise
  assert.equal(
    wire.sent.filter((sent) => sent.rpc.method === 'initialize').length,
    1,
  )
  await runtime.shutdown()
  assert.equal(
    wire.sent.filter((sent) => sent.rpc.method === 'initialize').length,
    1,
  )
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
      assert.equal(
        new Headers(init.headers).get('MCP-Protocol-Version'),
        '2024-11-05',
      )
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
  assert.equal(
    (await session.call('mcp_one_echo', {}, signal())).kind,
    'completed',
  )
  assert.equal(dispatchedCalls, 1)
  await runtime.shutdown()
})

test('oversized arguments are a definite failure and never dispatch tools/call', async () => {
  const wire = new Wire(() => {
    throw new Error('must not dispatch tools/call')
  })
  const runtime = new McpRuntime(wire, new Store([server()]))
  const session = await runtime.session(signal())
  const result = await session.call(
    'mcp_one_echo',
    { text: 'é'.repeat(MAX_MESSAGE_BYTES / 2) },
    signal(),
  )
  assert.equal(result.kind, 'failed')
  if (result.kind === 'failed') assert.equal(result.code, 'bad_request')
  assert.equal(
    wire.sent.filter((sent) => sent.rpc.method === 'tools/call').length,
    0,
  )
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
      const runtime = new McpRuntime(
        wire,
        new Store([server({ url, allowPrivate })]),
      )
      const session = await runtime.session(signal())
      const result = await session.call('mcp_one_echo', {}, signal())
      assert.equal(
        result.kind,
        allowPrivate ? 'completed' : 'failed',
        `${url}: allowPrivate=${String(allowPrivate)}`,
      )
      assert.equal(wire.sent.length, allowPrivate ? 3 : 0)
      await runtime.shutdown()
    }
  }
})
