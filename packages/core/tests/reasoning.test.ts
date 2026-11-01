import test from 'node:test'
import assert from 'node:assert/strict'
import { requestBody } from '../src/protocol/request.js'
import type { ModelRequest } from '../src/protocol/request.js'
import { Decoder } from '../src/protocol/decoder.js'
import { emptyUsage } from '../src/pricing.js'
import { at } from '../src/guard.js'
import { estimateContextTokens, contextUsageForRequest, validateReasoning } from '../src/engine/index.js'

function request(): ModelRequest {
  return {
    protocol: 'openai-chat',
    endpointId: 'endpoint',
    model: 'model',
    baseUrl: 'https://api.openai.com/v1',
    system: 'rules',
    history: [],
    input: 'question',
    maxOutput: 4096,
    reasoningEffort: 'high',
    reasoning: { dialect: 'openai', efforts: ['high'] },
    reasoningBudget: null,
  }
}
test('explicit dialect controls and vendor-only responses summary', () => {
  const current = request()
  assert.equal(requestBody(current).reasoning_effort, 'high')
  current.reasoning = { dialect: 'thinking-type', efforts: ['high'] }
  assert.equal(at(requestBody(current), 'thinking', 'type'), 'enabled')
  current.reasoning = { dialect: 'qwen', efforts: ['high'] }
  current.reasoningBudget = 2048
  assert.equal(requestBody(current).thinking_budget, 2048)
  current.protocol = 'anthropic-messages'
  current.reasoning = { dialect: 'anthropic-budget', efforts: ['high'], budgetTokens: 4096 }
  assert.equal(at(requestBody(current), 'thinking', 'budget_tokens'), 2048)
  current.reasoning = { dialect: 'anthropic-adaptive', efforts: ['high'] }
  assert.equal(at(requestBody(current), 'output_config', 'effort'), 'high')
  current.protocol = 'openai-responses'
  current.reasoning = { dialect: 'openai', efforts: ['high'] }
  assert.equal(at(requestBody(current), 'reasoning', 'summary'), 'auto')
  current.baseUrl = 'https://third-party.example/v1'
  assert.equal(at(requestBody(current), 'reasoning', 'summary'), undefined)
})
test('native search uses protocol-specific server tools and rejects Chat Completions', () => {
  const current = request()
  current.nativeWebSearch = true
  assert.throws(() => requestBody(current))
  current.protocol = 'anthropic-messages'
  assert.equal(at(requestBody(current), 'tools', 0, 'type'), 'web_search_20250305')
  assert.equal(at(requestBody(current), 'tools', 0, 'max_uses'), 5)
  current.protocol = 'openai-responses'
  assert.equal(at(requestBody(current), 'tools', 0, 'type'), 'web_search')
})
test('signed anthropic reasoning replays natively and encrypted responses are not displayed', () => {
  const usage = emptyUsage(),
    decoder = new Decoder('anthropic-messages', 'endpoint', 'model')
  decoder.event(
    JSON.stringify({ type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '' } }),
    usage,
  )
  assert.equal(
    decoder.event(
      JSON.stringify({ type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: 'visible' } }),
      usage,
    )?.reasoning,
    'visible',
  )
  decoder.event(
    JSON.stringify({ type: 'content_block_delta', index: 0, delta: { type: 'signature_delta', signature: 'signed' } }),
    usage,
  )
  decoder.event(JSON.stringify({ type: 'content_block_stop', index: 0 }), usage)
  assert.equal(at(decoder.reasoning[0]?.value, 'signature'), 'signed')
  const current = request()
  current.protocol = 'anthropic-messages'
  current.turn = [{ assistant: { text: 'answer', tool_calls: [], reasoning: decoder.reasoning } }]
  assert.equal(at(requestBody(current), 'messages', 1, 'content', 0, 'thinking'), 'visible')
  const responses = new Decoder('openai-responses', 'endpoint', 'model')
  assert.equal(
    responses.event(
      JSON.stringify({
        type: 'response.output_item.done',
        item: { type: 'reasoning', id: 'r', encrypted_content: 'secret', summary: [] },
      }),
      usage,
    ),
    null,
  )
  assert.equal(responses.reasoning.length, 1)
})

test('shared context distinguishes previews, per-request reports and unknown counts', () => {
  assert.equal(estimateContextTokens('中文😀'), 1)
  assert.equal(estimateContextTokens(undefined), null)
  const cyclic: { self?: unknown } = {}
  cyclic.self = cyclic
  assert.equal(estimateContextTokens(cyclic), null)
  const request = {
    inputTokens: 10,
    outputTokens: 5,
    estimatedInputTokens: 90,
    contextWindow: null,
    endpointId: 'endpoint',
    modelId: 'model',
  }
  assert.deepEqual(contextUsageForRequest(request), {
    tokens: 15,
    contextWindow: null,
    source: 'reported',
    endpointId: 'endpoint',
    modelId: 'model',
  })
  assert.equal(contextUsageForRequest({ ...request, inputTokens: null }).source, 'estimated')
  assert.equal(contextUsageForRequest({ ...request, inputTokens: null, estimatedInputTokens: null }).tokens, null)
  assert.equal(
    validateReasoning(
      { dialect: 'anthropic-budget', efforts: ['high'], budgetTokens: 2048 },
      'anthropic-messages',
      'high',
      4096,
    ),
    1638,
  )
  assert.throws(() =>
    validateReasoning(
      { dialect: 'anthropic-budget', efforts: ['high'], budgetTokens: 2048 },
      'openai-chat',
      'high',
      4096,
    ),
  )
  assert.throws(() => validateReasoning(null, 'openai-chat', 'high', 4096))
})
