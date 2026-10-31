import assert from 'node:assert/strict'
import { test } from 'node:test'
import { defaultPreferences, type ResolvedSearchConfig } from './index.js'
import {
  fetchRequest,
  parseMcp,
  parsePage,
  parseSearchResponse,
  searchRequest,
} from './vendor.js'

const config = (provider = 'tavily'): ResolvedSearchConfig => ({
  provider,
  baseUrl: 'https://api.example.test',
  apiKeys: ['first', 'second'],
  preferences: defaultPreferences(),
})
test('all provider paths, authentication variants, and response aliases remain independent', () => {
  const cases: readonly [string, string, string, string][] = [
    ['zhipu', '', 'authorization', 'Bearer first'],
    ['tavily', '/search', 'authorization', 'Bearer first'],
    [
      'searxng',
      '/search?q=query&format=json&language=auto',
      'authorization',
      'Basic dXNlcjpmaXJzdA==',
    ],
    ['exa', '/search', 'x-api-key', 'first'],
    ['exa-mcp', '', 'x-api-key', 'first'],
    ['bocha', '/v1/web-search', 'authorization', 'Bearer first'],
    ['querit', '/v1/search', 'authorization', 'Bearer first'],
    [
      'brave',
      '/res/v1/web/search?q=query&count=5',
      'x-subscription-token',
      'first',
    ],
    ['parallel', '/v1/search', 'x-api-key', 'first'],
    ['jina', '/query', 'authorization', 'Bearer first'],
    ['firecrawl', '/v2/search', 'authorization', 'Bearer first'],
  ]
  for (const [provider, suffix, header, expected] of cases) {
    const request = searchRequest(
      { ...config(provider), basicAuthUsername: 'user' },
      'query',
      'first',
    )
    assert.equal(request.url, 'https://api.example.test' + suffix)
    assert.equal(new Headers(request.init.headers).get(header), expected)
  }
  const item = {
    name: 'Title',
    link: 'https://source.test/',
    excerpts: ['first', 'second'],
  }
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
    assert.throws(
      () =>
        parseSearchResponse(
          provider,
          JSON.stringify({ code: 400, error_code: 400, success: false }),
        ),
      {
        code: 'upstream',
      },
    )
  assert.equal(
    parsePage(
      'querit',
      JSON.stringify({
        data: [{ metadata: { title: 'Title' }, markdown: 'Body' }],
      }),
      'https://source.test/',
    ).content,
    'Body',
  )
})

test('each fetch protocol preserves target body and authentication semantics', () => {
  const target = 'https://source.test/page?q=one'
  const querit = fetchRequest(config('querit'), target, 'key')
  assert.equal(querit.url, 'https://api.example.test/v1/contents')
  assert.deepEqual(JSON.parse(String(querit.init.body)), {
    urls: [target],
    format: 'markdown',
    extrasMeta: true,
  })
  const jina = fetchRequest(config('jina'), target, undefined)
  assert.equal(jina.url, 'https://api.example.test/' + target)
  assert.equal(new Headers(jina.init.headers).get('x-retain-images'), 'none')
  assert.equal(new Headers(jina.init.headers).has('authorization'), false)
  const firecrawl = fetchRequest(config('firecrawl'), target, 'key')
  assert.equal(firecrawl.url, 'https://api.example.test/v2/scrape')
  assert.deepEqual(JSON.parse(String(firecrawl.init.body)), {
    url: target,
    formats: ['markdown'],
  })
  assert.equal(
    new Headers(firecrawl.init.headers).get('authorization'),
    'Bearer key',
  )
  assert.equal(
    parsePage(
      'jina',
      JSON.stringify({
        data: { title: 'Title', markdown: '😀'.repeat(24001) },
      }),
      target,
    ).content,
    '😀'.repeat(24000),
  )
  assert.equal(
    parsePage(
      'firecrawl',
      JSON.stringify({
        success: true,
        data: {
          markdown: 'Body',
          metadata: { title: 'Title', sourceURL: target },
        },
      }),
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
    parsePage(
      'fetch',
      '<title>Fragment title</title><p>Visible</p><!--noise--><style>noise</style>',
      target,
    ),
    { title: 'Fragment title', url: target, content: 'Visible' },
  )
})

test('Exa MCP JSON and SSE retain multiple Highlights paragraphs and reject RPC errors', () => {
  const content =
    'Title: First\nURL: https://one.test/\nHighlights: first paragraph\n\nsecond paragraph\nTitle: Second\nURL: https://two.test/\nText: other'
  const envelope = {
    jsonrpc: '2.0',
    id: 1,
    result: { content: [{ type: 'text', text: content }] },
  }
  const expected = [
    {
      title: 'First',
      url: 'https://one.test/',
      snippet: 'first paragraph\n\nsecond paragraph',
    },
    { title: 'Second', url: 'https://two.test/', snippet: 'other' },
  ]
  assert.deepEqual(parseMcp(JSON.stringify(envelope)), expected)
  assert.deepEqual(
    parseMcp(
      'event: message\r\ndata: ' + JSON.stringify(envelope) + '\r\n\r\n',
    ),
    expected,
  )
  assert.throws(() => parseMcp('{"error":{"code":-1}}'), { code: 'upstream' })
  assert.throws(() => parseMcp('{"result":{"isError":true,"content":[]}}'), {
    code: 'upstream',
  })
})
