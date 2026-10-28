import assert from 'node:assert/strict'
import { test } from 'node:test'
import { KitFailure } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import { defaultPreferences, fetch, search, validatePreferences, type ResolvedSearchConfig } from './index.js'
import { fetchRequest, parseMcp, parsePage, parseSearchResponse, searchRequest } from './vendor.js'

const config = (provider = 'tavily'): ResolvedSearchConfig => ({
  provider,
  baseUrl: 'https://api.example.test',
  apiKeys: ['first', 'second'],
  preferences: defaultPreferences(),
})
const hits = () =>
  Response.json({
    results: [
      { title: 'First', url: 'https://good.test/page', content: '😀'.repeat(150) },
      { title: 'Duplicate', url: 'https://good.test/page', content: 'duplicate' },
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
        preferences: { ...defaultPreferences(), compression: 'cutoff', cutoffLimit: 100, blacklist: ['BLOCKED.TEST.'] },
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
        : new Response('<title>Title</title><script>evil()</script><p>Hello world</p>')
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
  assert.deepEqual(sent, ['https://public.test/start', 'https://public.test/next'])
  assert.deepEqual(checked, sent)
  assert.deepEqual(result, { title: 'Title', url: 'https://public.test/next', content: 'Hello world' })
  let calls = 0
  await assert.rejects(
    fetch(
      { ...config('fetch'), preferences: { ...defaultPreferences(), blacklist: ['blocked.test'] } },
      'https://public.test/',
      {
        async fetch() {
          calls++
          return new Response(null, { status: 302, headers: { location: 'https://blocked.test/' } })
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
          return new Response(null, { status: 302, headers: { location: '/again' } })
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
      return Response.json({ data: { url: 'https://blocked.test/', markdown: 'content' } })
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
      { ...config('jina'), preferences: { ...defaultPreferences(), blacklist: ['blocked.test'] } },
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
        return new Response('x', { headers: { 'content-length': String(2 * 1024 * 1024 + 1) } })
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

test('all provider paths, authentication variants, and response aliases remain independent', () => {
  const cases: readonly [string, string, string, string][] = [
    ['zhipu', '', 'authorization', 'Bearer first'],
    ['tavily', '/search', 'authorization', 'Bearer first'],
    ['searxng', '/search?q=query&format=json&language=auto', 'authorization', 'Basic dXNlcjpmaXJzdA=='],
    ['exa', '/search', 'x-api-key', 'first'],
    ['exa-mcp', '', 'x-api-key', 'first'],
    ['bocha', '/v1/web-search', 'authorization', 'Bearer first'],
    ['querit', '/v1/search', 'authorization', 'Bearer first'],
    ['brave', '/res/v1/web/search?q=query&count=5', 'x-subscription-token', 'first'],
    ['parallel', '/v1/search', 'x-api-key', 'first'],
    ['jina', '/query', 'authorization', 'Bearer first'],
    ['firecrawl', '/v2/search', 'authorization', 'Bearer first'],
  ]
  for (const [provider, suffix, header, expected] of cases) {
    const request = searchRequest({ ...config(provider), basicAuthUsername: 'user' }, 'query', 'first')
    assert.equal(request.url, 'https://api.example.test' + suffix)
    assert.equal(new Headers(request.init.headers).get(header), expected)
  }
  const item = { name: 'Title', link: 'https://source.test/', excerpts: ['first', 'second'] }
  const payloads: Readonly<Record<string, unknown>> = {
    zhipu: { search_result: [item] },
    tavily: { results: [item] },
    searxng: { results: [item] },
    exa: { results: [item] },
    bocha: { code: 200, data: { webPages: { value: [item] } } },
    querit: { error_code: 200, results: { result: [item] } },
    brave: { web: { results: [item] } },
    parallel: { results: [item] },
    jina: { data: [item] },
    firecrawl: { success: true, data: { web: [item] } },
  }
  for (const [provider, payload] of Object.entries(payloads))
    assert.deepEqual(parseSearchResponse(provider, JSON.stringify(payload)), [
      { title: 'Title', url: 'https://source.test/', snippet: 'first\nsecond' },
    ])
  for (const provider of ['bocha', 'querit', 'firecrawl'])
    assert.throws(() => parseSearchResponse(provider, JSON.stringify({ code: 400, error_code: 400, success: false })), {
      code: 'upstream',
    })
  assert.equal(
    parsePage(
      'querit',
      JSON.stringify({ data: [{ metadata: { title: 'Title' }, markdown: 'Body' }] }),
      'https://source.test/',
    ).content,
    'Body',
  )
})

test('each fetch protocol preserves target body and authentication semantics', () => {
  const target = 'https://source.test/page?q=one'
  const querit = fetchRequest(config('querit'), target, 'key')
  assert.equal(querit.url, 'https://api.example.test/v1/contents')
  assert.deepEqual(JSON.parse(String(querit.init.body)), { urls: [target], format: 'markdown', extrasMeta: true })
  const jina = fetchRequest(config('jina'), target, undefined)
  assert.equal(jina.url, 'https://api.example.test/' + target)
  assert.equal(new Headers(jina.init.headers).get('x-retain-images'), 'none')
  assert.equal(new Headers(jina.init.headers).has('authorization'), false)
  const firecrawl = fetchRequest(config('firecrawl'), target, 'key')
  assert.equal(firecrawl.url, 'https://api.example.test/v2/scrape')
  assert.deepEqual(JSON.parse(String(firecrawl.init.body)), { url: target, formats: ['markdown'] })
  assert.equal(new Headers(firecrawl.init.headers).get('authorization'), 'Bearer key')
  assert.equal(
    parsePage('jina', JSON.stringify({ data: { title: 'Title', markdown: '😀'.repeat(24001) } }), target).content,
    '😀'.repeat(24000),
  )
  assert.equal(
    parsePage(
      'firecrawl',
      JSON.stringify({ success: true, data: { markdown: 'Body', metadata: { title: 'Title', sourceURL: target } } }),
      'https://other.test/',
    ).url,
    target,
  )
})

test('static HTML extraction excludes document metadata and non-visible blocks for documents and fragments', () => {
  const target = 'https://source.test/'
  assert.deepEqual(
    parsePage(
      'fetch',
      '<html><head><title>Page title</title><style>noise</style></head>outside<body><p>Visible</p><script>noise</script><noscript>noise</noscript></body>outside</html>',
      target,
    ),
    { title: 'Page title', url: target, content: 'Visible' },
  )
  assert.deepEqual(
    parsePage('fetch', '<title>Fragment title</title><p>Visible</p><!--noise--><style>noise</style>', target),
    { title: 'Fragment title', url: target, content: 'Visible' },
  )
})

test('Exa MCP JSON and SSE retain multiple Highlights paragraphs and reject RPC errors', () => {
  const content =
    'Title: First\nURL: https://one.test/\nHighlights: first paragraph\n\nsecond paragraph\nTitle: Second\nURL: https://two.test/\nText: other'
  const envelope = { jsonrpc: '2.0', id: 1, result: { content: [{ type: 'text', text: content }] } }
  const expected = [
    { title: 'First', url: 'https://one.test/', snippet: 'first paragraph\n\nsecond paragraph' },
    { title: 'Second', url: 'https://two.test/', snippet: 'other' },
  ]
  assert.deepEqual(parseMcp(JSON.stringify(envelope)), expected)
  assert.deepEqual(parseMcp('event: message\r\ndata: ' + JSON.stringify(envelope) + '\r\n\r\n'), expected)
  assert.throws(() => parseMcp('{"error":{"code":-1}}'), { code: 'upstream' })
  assert.throws(() => parseMcp('{"result":{"isError":true,"content":[]}}'), { code: 'upstream' })
})

test('public boundaries reject invalid preferences, unsupported capabilities, and missing credentials', async () => {
  for (const rule of ['bad[host', 'https://user@host/path', 'host:80', 'https://host:99999/'])
    assert.throws(() => validatePreferences({ ...defaultPreferences(), blacklist: [rule] }), { code: 'bad_request' })
  const http: OutboundHttp = {
    async fetch() {
      throw new Error('must not dispatch')
    },
  }
  await assert.rejects(search(config('fetch'), 'query', http), { code: 'bad_request' })
  await assert.rejects(search({ ...config(), apiKeys: [] }, 'query', http), { code: 'needs_config' })
  await assert.rejects(search(config(), '😀'.repeat(4097), http), { code: 'bad_request' })
  await assert.rejects(
    fetch(config('fetch'), 'https://public.test/' + '😀'.repeat(3000), http, async () => {}),
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
          ? fetch(config(provider), 'https://public.test/', http, async () => {})
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
