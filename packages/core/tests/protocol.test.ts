import test from 'node:test'
import assert from 'node:assert/strict'
import { SseParser } from '../src/sse.js'
import { requestBody } from '../src/protocol/request.js'
import { cost, emptyUsage } from '../src/pricing.js'
import { within, shellRisk } from '../src/risk.js'
test('SSE accepts all UTF-8 and CRLF splits and rejects unbounded input', () => {
  const bytes = new TextEncoder().encode(
    'event: text\r\ndata: 你好\r\ndata: world\r\n\r\n',
  )
  for (let split = 0; split <= bytes.length; split++) {
    const parser = new SseParser()
    assert.deepEqual(
      [
        ...parser.feed(bytes.subarray(0, split)),
        ...parser.feed(bytes.subarray(split)),
        ...parser.finish(),
      ],
      [{ event: 'text', data: '你好\nworld' }],
    )
  }
  assert.throws(() =>
    new SseParser(8, 16).feed(new TextEncoder().encode('data: 123456789')),
  )
})
test('protocols encode current and historical images without replaying mismatched reasoning', () => {
  for (const protocol of [
    'anthropic-messages',
    'openai-chat',
    'openai-responses',
  ] as const) {
    const body = requestBody({
      protocol,
      model: 'model',
      endpointId: 'endpoint',
      baseUrl: 'https://example.com/v1',
      system: '',
      input: 'now',
      history: [
        { user: 'before', images: [{ mime: 'image/png', data: 'aGVsbG8=' }] },
        {
          assistant: {
            text: 'answer',
            tool_calls: [],
            reasoning: [
              {
                protocol,
                endpointId: 'different',
                modelId: 'model',
                value: { type: 'thinking', thinking: 'secret' },
              },
            ],
          },
        },
      ],
      images: [{ mime: 'image/png', data: 'aGVsbG8=' }],
      maxOutput: 4096,
      reasoningEffort: 'default',
      reasoning: null,
      reasoningBudget: null,
    })
    const encoded = JSON.stringify(body)
    assert.equal((encoded.match(/aGVsbG8=/g) ?? []).length, 2)
    assert.ok(!encoded.includes('secret'))
  }
})
test('cost uses per-request tiers and caches, lexical risk is advisory', () => {
  const usage = {
    ...emptyUsage(),
    inputTokens: 100,
    outputTokens: 20,
    cacheReadTokens: 40,
  }
  assert.deepEqual(
    cost(
      {
        currency: 'USD',
        tiers: [
          {
            minInputTokens: 0,
            input: 2,
            output: 4,
            cacheRead: 1,
            cacheWrite: null,
          },
        ],
      },
      [usage],
    ),
    { currency: 'USD', amount: 0.00024 },
  )
  assert.equal(cost(null, [usage]), null)
  assert.equal(within('C:\\Safe\\Child\\..\\file', ['c:\\safe']), true)
  assert.equal(within('/safe/../unsafe', ['/safe']), false)
  assert.equal(within('relative', ['/']), false)
  assert.equal(shellRisk('sudo rm -rf /tmp/x')?.reason, 'elevation')
})
