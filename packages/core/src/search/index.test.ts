import assert from 'node:assert/strict'
import { test } from 'node:test'
import { KitFailure } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import {
  defaultPreferences,
  fetch,
  search,
  validatePreferences,
  type ResolvedSearchConfig,
} from './index.js'

const config = (provider = 'tavily'): ResolvedSearchConfig => ({
  provider,
  baseUrl: 'https://api.example.test',
  apiKeys: ['first', 'second'],
  preferences: defaultPreferences(),
})
const hits = () =>
  Response.json({
    results: [
      {
        title: 'First',
        url: 'https://good.test/page',
        content: '😀'.repeat(150),
      },
      {
        title: 'Duplicate',
        url: 'https://good.test/page',
        content: 'duplicate',
      },
      { url: 'https://blocked.test/page', content: 'blocked' },
      { url: 'file:///etc/passwd', content: 'invalid' },
    ],
  })

test('key fallback is limited to 401/403/429 and applies strict Unicode policy after deduplication', async () => {
  for (const status of [401, 403, 429]) {
    const sent: string[] = []
    const outbound: OutboundHttp = {
      async fetch(_url, init) {
        sent.push(new Headers(init.headers).get('authorization') ?? '')
        assert.equal(init.redirect, 'manual')
        return sent.length === 1 ? new Response('rejected', { status }) : hits()
      },
    }
    const result = await search(
      {
        ...config(),
        preferences: {
          ...defaultPreferences(),
          compression: 'cutoff',
          cutoffLimit: 100,
          blacklist: ['BLOCKED.TEST.'],
        },
      },
      '  query 😀  ',
      outbound,
    )
    assert.deepEqual(sent, ['Bearer first', 'Bearer second'])
    assert.equal(result.query, 'query 😀')
    assert.equal(result.hits.length, 1)
    assert.equal(result.hits[0]?.snippet, '😀'.repeat(100))
  }
  for (const status of [400, 404, 500, 503]) {
    let sent = 0
    await assert.rejects(
      search(config(), 'query', {
        async fetch() {
          sent++
          return new Response('failure', { status })
        },
      }),
      { code: 'upstream' },
    )
    assert.equal(sent, 1)
  }
  let sent = 0
  await assert.rejects(
    search(config(), 'query', {
      async fetch() {
        sent++
        throw new Error('secret-key')
      },
    }),
    { code: 'network', message: 'Search provider request failed' },
  )
  assert.equal(sent, 1)
})

test('direct fetch omits every provider credential and validates every redirect before dispatch', async () => {
  const sent: string[] = []
  const checked: string[] = []
  const outbound: OutboundHttp = {
    async fetch(url, init) {
      sent.push(url)
      assert.equal(new Headers(init.headers).has('authorization'), false)
      assert.equal(new Headers(init.headers).has('x-api-key'), false)
      assert.equal(init.credentials, 'omit')
      return sent.length === 1
        ? new Response(null, { status: 302, headers: { location: '/next' } })
        : new Response(
            '<title>Title</title><script>evil()</script><p>Hello world</p>',
          )
    },
  }
  const result = await fetch(
    { ...config('fetch'), basicAuthUsername: 'private-user' },
    'https://public.test/start',
    outbound,
    async (url) => {
      checked.push(url)
    },
  )
  assert.deepEqual(sent, [
    'https://public.test/start',
    'https://public.test/next',
  ])
  assert.deepEqual(checked, sent)
  assert.deepEqual(result, {
    title: 'Title',
    url: 'https://public.test/next',
    content: 'Hello world',
  })
  let calls = 0
  await assert.rejects(
    fetch(
      {
        ...config('fetch'),
        preferences: { ...defaultPreferences(), blacklist: ['blocked.test'] },
      },
      'https://public.test/',
      {
        async fetch() {
          calls++
          return new Response(null, {
            status: 302,
            headers: { location: 'https://blocked.test/' },
          })
        },
      },
      async () => {},
    ),
    { code: 'bad_request' },
  )
  assert.equal(calls, 1)
  calls = 0
  await assert.rejects(
    fetch(
      config('fetch'),
      'https://public.test/',
      {
        async fetch() {
          calls++
          return new Response(null, {
            status: 302,
            headers: { location: '/again' },
          })
        },
      },
      async () => {},
    ),
    { code: 'upstream' },
  )
  assert.equal(calls, 6)
})

test('direct fetch does not retry unauthorized pages using unrelated provider keys', async () => {
  let calls = 0
  await assert.rejects(
    fetch(
      config('fetch'),
      'https://public.test/',
      {
        async fetch() {
          calls++
          return new Response('unauthorized', { status: 401 })
        },
      },
      async () => {},
    ),
    { code: 'upstream' },
  )
  assert.equal(calls, 1)
})

test('proxy fetch rejects denied target before contacting provider and checks returned source', async () => {
  let sent = 0
  const http: OutboundHttp = {
    async fetch() {
      sent++
      return Response.json({
        data: { url: 'https://blocked.test/', markdown: 'content' },
      })
    },
  }
  await assert.rejects(
    fetch(config('jina'), 'https://private.test/', http, async () => {
      throw new KitFailure('bad_request', 'Denied target')
    }),
    { code: 'bad_request' },
  )
  assert.equal(sent, 0)
  await assert.rejects(
    fetch(
      {
        ...config('jina'),
        preferences: { ...defaultPreferences(), blacklist: ['blocked.test'] },
      },
      'https://public.test/',
      http,
      async () => {},
    ),
    { code: 'bad_request' },
  )
  assert.equal(sent, 1)
})

test('response reader bounds both declared and chunked data without retrying keys', async () => {
  let calls = 0
  await assert.rejects(
    search(config(), 'query', {
      async fetch() {
        calls++
        return new Response('x', {
          headers: { 'content-length': String(2 * 1024 * 1024 + 1) },
        })
      },
    }),
    { code: 'upstream' },
  )
  assert.equal(calls, 1)
  let cancelled = false
  await assert.rejects(
    search(config(), 'query', {
      async fetch() {
        return new Response(
          new ReadableStream<Uint8Array>({
            pull(controller) {
              controller.enqueue(new Uint8Array(1024 * 1024))
            },
            cancel() {
              cancelled = true
            },
          }),
        )
      },
    }),
    { code: 'upstream' },
  )
  assert.equal(cancelled, true)
  calls = 0
  await assert.rejects(
    search(config(), 'query', {
      async fetch() {
        calls++
        return new Response('x'.repeat(8193), { status: 500 })
      },
    }),
    { code: 'upstream' },
  )
  assert.equal(calls, 1)
})

test('cancellation never dispatches a following key', async () => {
  const controller = new AbortController()
  let calls = 0
  await assert.rejects(
    search(
      config(),
      'query',
      {
        async fetch() {
          calls++
          controller.abort()
          return new Response('no', { status: 401 })
        },
      },
      controller.signal,
    ),
    { code: 'cancelled' },
  )
  assert.equal(calls, 1)
})

test('public boundaries reject invalid preferences, unsupported capabilities, and missing credentials', async () => {
  for (const rule of [
    'bad[host',
    'https://user@host/path',
    'host:80',
    'https://host:99999/',
  ])
    assert.throws(
      () => validatePreferences({ ...defaultPreferences(), blacklist: [rule] }),
      { code: 'bad_request' },
    )
  const http: OutboundHttp = {
    async fetch() {
      throw new Error('must not dispatch')
    },
  }
  await assert.rejects(search(config('fetch'), 'query', http), {
    code: 'bad_request',
  })
  await assert.rejects(search({ ...config(), apiKeys: [] }, 'query', http), {
    code: 'needs_config',
  })
  await assert.rejects(search(config(), '😀'.repeat(4097), http), {
    code: 'bad_request',
  })
  await assert.rejects(
    fetch(
      config('fetch'),
      'https://public.test/' + '😀'.repeat(3000),
      http,
      async () => {},
    ),
    { code: 'bad_request' },
  )
})

test('one total deadline covers response reading for search, Exa MCP, and fetch', async (context) => {
  for (const [provider, deadline] of [
    ['tavily', 15000],
    ['exa-mcp', 25000],
    ['fetch', 20000],
  ] as const)
    await context.test(provider, async (subtest) => {
      subtest.mock.timers.enable({ apis: ['setTimeout'] })
      const reading = Promise.withResolvers<void>()
      let dispatched = 0
      const http: OutboundHttp = {
        async fetch() {
          dispatched++
          return new Response(
            new ReadableStream<Uint8Array>({
              pull() {
                reading.resolve()
                return Promise.withResolvers<void>().promise
              },
            }),
          )
        },
      }
      const pending =
        provider === 'fetch'
          ? fetch(
              config(provider),
              'https://public.test/',
              http,
              async () => {},
            )
          : search(config(provider), 'query', http)
      const assertion = assert.rejects(pending, { code: 'timeout' })
      await reading.promise
      subtest.mock.timers.tick(deadline)
      await assertion
      assert.equal(dispatched, 1)
    })
})

test('connection timeout is ten seconds and never falls through to another key', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] })
  let dispatched = 0
  const pending = search(config(), 'query', {
    fetch() {
      dispatched++
      return Promise.withResolvers<Response>().promise
    },
  })
  const assertion = assert.rejects(pending, { code: 'timeout' })
  context.mock.timers.tick(10000)
  await assertion
  assert.equal(dispatched, 1)
})
