import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { Model } from '@ouraihub/aether-contracts'
import {
  prepare,
  validateHeaders,
  type EndpointSnapshot,
} from './connection.js'
import { inferModel, inferReasoning } from './infer.js'
import { probe } from './probe.js'

const endpoint: EndpointSnapshot = {
  endpointId: 'e',
  baseUrl: 'https://provider.example',
  protocol: 'openai-chat',
  keys: ['first-secret', 'second-secret'],
  headers: { 'x-team': 'shared' },
}
const model: Model = {
  id: 'gpt-5',
  modelType: 'chat',
  typeSource: 'inferred',
  contextWindow: 128000,
  maxOutput: 4096,
  canVision: true,
  supportsTools: true,
  reasoning: null,
  nativeWebSearch: false,
  name: null,
  groupName: null,
  pricing: null,
}
test('prepare enforces headers, reasoning and endpoint overrides without rotating explicit keys', () => {
  const connection = prepare({
    endpoint: { ...endpoint, imageBaseUrl: 'https://images.example/' },
    model,
  })
  assert.equal(connection.baseUrl, 'https://provider.example/v1')
  assert.equal(connection.imageBaseUrl, 'https://images.example/v1')
  assert.deepEqual(connection.keys, endpoint.keys)
  assert.throws(() => validateHeaders({ Authorization: 'secret' }), /reserved/)
  assert.throws(
    () => validateHeaders({ ' X-Team': 'a', 'x-team': 'b' }),
    /invalid/,
  )
  assert.throws(
    () => prepare({ endpoint, model: { ...model, nativeWebSearch: true } }),
    /unsupported_protocol/,
  )
  assert.throws(
    () => prepare({ endpoint, model: { ...model, maxOutput: 128000 } }),
    /exceeds_context/,
  )
  assert.throws(
    () =>
      prepare({
        endpoint,
        model: {
          ...model,
          reasoning: {
            dialect: 'anthropic-budget',
            efforts: ['high'],
            budgetTokens: 2048,
          },
        },
      }),
    /protocol_mismatch/,
  )
})
test('model inference preserves unknowns and official reasoning dialects', () => {
  assert.equal(inferModel('Vendor/qwen-vl-max').canVision, true)
  assert.equal(inferModel('unrecognized').supportsTools, null)
  assert.equal(inferModel('gpt-image-1').modelType, 'image')
  assert.equal(
    inferReasoning(
      'claude-opus-4.6',
      'anthropic-messages',
      'https://api.example',
      4096,
    )?.dialect,
    'anthropic-adaptive',
  )
  assert.equal(
    inferReasoning(
      'qwen3-max',
      'openai-chat',
      'https://dashscope.aliyuncs.com',
      4096,
    )?.dialect,
    'qwen',
  )
  assert.equal(
    inferReasoning('qwen3-max', 'openai-chat', 'https://relay.example', 4096),
    null,
  )
})
test('probe fixes key order, auth and header precedence and narrows upstream models', async () => {
  const requests: Array<{ url: string; headers: Headers }> = []
  const result = await probe(
    {
      async fetch(url, init) {
        requests.push({ url, headers: new Headers(init.headers) })
        return requests.length === 1
          ? new Response('first-secret denied', { status: 401 })
          : Response.json({
              data: [{ id: 'gpt-5' }, { id: 'gpt-5' }, { id: 'qwen-vl-max' }],
            })
      },
    },
    { endpoint, signal: new AbortController().signal },
  )
  assert.equal(requests.length, 2)
  assert.equal(requests[0]?.headers.get('authorization'), 'Bearer first-secret')
  assert.equal(
    requests[1]?.headers.get('authorization'),
    'Bearer second-secret',
  )
  assert.equal(requests[1]?.headers.get('x-team'), 'shared')
  assert.equal(result.error, null)
  assert.deepEqual(
    result.models.map((item) => item.id),
    ['gpt-5', 'qwen-vl-max'],
  )
})
test('probe never falls back on 5xx and masks all fixed credentials', async () => {
  let count = 0
  const result = await probe(
    {
      async fetch() {
        count++
        return new Response('first-secret second-secret failed', {
          status: 503,
        })
      },
    },
    { endpoint, signal: new AbortController().signal },
  )
  assert.equal(count, 1)
  assert.equal(result.error?.kind, 'status')
  assert.equal(result.error?.message, '*** *** failed')
})
test('probe rejects oversized streamed body and cancellation does not try another key', async () => {
  const large = await probe(
    {
      async fetch() {
        return new Response('x', {
          headers: { 'content-length': String(5 * 1024 * 1024) },
        })
      },
    },
    { endpoint, signal: new AbortController().signal },
  )
  assert.equal(large.error?.kind, 'invalid_body')
  const abort = new AbortController()
  let count = 0
  await assert.rejects(
    probe(
      {
        async fetch() {
          count++
          abort.abort()
          throw new DOMException('Cancelled', 'AbortError')
        },
      },
      { endpoint, signal: abort.signal },
    ),
    /Cancelled/,
  )
  assert.equal(count, 1)
})
test('Messages protocol probe sends both authentication headers and suggests only actual listed Claude', async () => {
  const requests: Array<{ url: string; headers: Headers; body: unknown }> = []
  const result = await probe(
    {
      async fetch(url, init) {
        requests.push({
          url,
          headers: new Headers(init.headers),
          body: typeof init.body === 'string' ? JSON.parse(init.body) : null,
        })
        return requests.length === 1
          ? Response.json({ models: [{ name: 'claude-sonnet-4-6' }] })
          : new Response(null, { status: 422 })
      },
    },
    { endpoint, signal: new AbortController().signal },
  )
  assert.equal(result.suggestedProtocol, 'anthropic-messages')
  assert.equal(requests[1]?.headers.get('x-api-key'), 'first-secret')
  assert.equal(requests[1]?.headers.get('authorization'), 'Bearer first-secret')
  assert.equal(requests[1]?.headers.get('anthropic-version'), '2023-06-01')
  assert.deepEqual(requests[1]?.body, {
    model: 'claude-sonnet-4-6',
    max_tokens: 1,
    messages: [],
  })
})
