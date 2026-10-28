import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import type { Protocol, RunEvent } from '@ouraihub/aether-contracts'
import { RunEngine } from '../src/engine/index.js'
import type { RunInput, ToolExecution, ToolSpec } from '../src/types.js'
import { DEFAULT_BUDGET } from '../src/types.js'
import type { ArgumentsValidator, OutboundHttp } from '../src/ports.js'
import { SseParser } from '../src/sse.js'
import { requestBody } from '../src/protocol/request.js'
import { cost, emptyUsage } from '../src/pricing.js'
import { within, shellRisk } from '../src/risk.js'

const schemaArguments: ArgumentsValidator = (schema, args) =>
  schema.type === 'object' && typeof args.value === 'string' && Object.keys(args).every((key) => key === 'value')
const tool: ToolSpec = {
  name: 'read_value',
  description: 'Reads a value',
  parameters: {
    type: 'object',
    properties: { value: { type: 'string' } },
    required: ['value'],
    additionalProperties: false,
  },
  source: { kind: 'host' },
  effect: 'read',
  approval: 'auto',
}
function input(protocol: Protocol = 'openai-chat'): RunInput {
  return {
    runId: 'run',
    purpose: 'chat',
    permissionMode: 'default',
    connection: {
      protocol,
      endpointId: 'endpoint',
      modelId: 'model',
      baseUrl: 'https://models.example/v1',
      keys: ['key-one', 'key-two'],
      headers: {},
      supportsTools: true,
      canVision: false,
      maxOutput: null,
      contextWindow: null,
      reasoning: null,
      pricing: null,
    },
    system: [{ kind: 'host', text: 'Be useful' }],
    history: [],
    input: 'Hello',
    images: [],
    tools: [],
    budget: { ...DEFAULT_BUDGET },
    reasoningEffort: 'default',
    nativeWebSearch: false,
  }
}
function sse(events: readonly unknown[]): Response {
  const encoded = new TextEncoder().encode(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(''))
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        for (let i = 0; i < encoded.length; i += 7) controller.enqueue(encoded.subarray(i, i + 7))
        controller.close()
      },
    }),
    { headers: { 'content-type': 'text/event-stream' } },
  )
}
function textResponse(protocol: Protocol): Response {
  if (protocol === 'anthropic-messages')
    return sse([
      {
        type: 'message_start',
        message: { usage: { input_tokens: 10, cache_read_input_tokens: 3, cache_creation_input_tokens: 2 } },
      },
      { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: '你好' } },
      { type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage: { output_tokens: 4 } },
      { type: 'message_stop' },
    ])
  if (protocol === 'openai-responses')
    return sse([
      { type: 'response.output_text.delta', delta: '你好' },
      { type: 'response.completed', response: { usage: { input_tokens: 15, output_tokens: 4 } } },
    ])
  return sse([
    { choices: [{ delta: { content: '你好' }, finish_reason: null }] },
    { choices: [{ delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 15, completion_tokens: 4 } },
  ])
}

test('terminal text-only token limits preserve the reply without replaying a model request', async () => {
  for (const protocol of ['anthropic-messages', 'openai-chat', 'openai-responses'] as const) {
    const bytes = await readFile(new URL(`../../ui/fixtures/sse/${protocol}/truncated.sse`, import.meta.url))
    let requests = 0
    const events: RunEvent[] = []
    const engine = new RunEngine({
      outbound: {
        fetch: async () => {
          requests++
          return new Response(bytes)
        },
      },
      tools: {
        authorize: () => 'deny',
        execute: async () => {
          throw new Error('No tools allowed')
        },
      },
      approver: { wait: async () => false },
      validateArguments: schemaArguments,
    })
    const result = await engine.run(input(protocol), new AbortController().signal, (event) => events.push(event))
    assert.equal(result.status, 'completed')
    assert.equal(requests, 1)
    assert.equal(events.filter((event) => event.kind === 'end').length, 1)
    assert.ok(result.messages.some((message) => 'assistant' in message && message.assistant.text))
  }
})
function callsResponse(calls: readonly { id: string; value: unknown }[], truncated = false): Response {
  return sse([
    {
      choices: [
        {
          delta: {
            tool_calls: calls.map((call, index) => ({
              index,
              id: call.id,
              function: { name: tool.name, arguments: JSON.stringify({ value: call.value }) },
            })),
          },
          finish_reason: null,
        },
      ],
    },
    {
      choices: [{ delta: {}, finish_reason: truncated ? 'length' : 'tool_calls' }],
      usage: { prompt_tokens: 12, completion_tokens: 2 },
    },
  ])
}
function engine(
  outbound: OutboundHttp,
  execute: () => Promise<ToolExecution> = async () => ({ kind: 'ok', text: 'value' }),
  approval: 'auto' | 'ask' | 'deny' = 'auto',
  wait: () => Promise<boolean> = async () => true,
): RunEngine {
  return new RunEngine({
    outbound,
    validateArguments: schemaArguments,
    tools: { authorize: () => approval, execute },
    approver: { wait },
  })
}
for (const protocol of ['openai-chat', 'openai-responses', 'anthropic-messages'] as const) {
  test(`${protocol}: real chunked stream, usage and one final End`, async () => {
    const events: RunEvent[] = [],
      requests: RequestInit[] = []
    const summary = await engine({
      fetch: async (_url, init) => {
        requests.push(init)
        return textResponse(protocol)
      },
    }).run(input(protocol), new AbortController().signal, (event) => events.push(event))
    assert.equal(summary.status, 'completed')
    assert.equal(summary.usage.inputTokens, 15)
    assert.equal(events.filter((event) => event.kind === 'end').length, 1)
    assert.equal(events.at(-1)?.kind, 'end')
    assert.equal(
      events
        .filter((event) => event.kind === 'text_delta')
        .map((event) => event.text)
        .join(''),
      '你好',
    )
    assert.equal(requests[0]?.redirect, 'manual')
  })
}
test('catalogue deny cannot be relaxed; approval deny and invalid arguments never execute', async () => {
  for (const variant of ['catalogue', 'approval', 'schema'] as const) {
    let round = 0,
      executions = 0,
      authorizations = 0
    const run = input()
    run.tools = [{ ...tool, approval: variant === 'catalogue' ? 'deny' : 'ask' }]
    const events: RunEvent[] = []
    const runner = new RunEngine({
      outbound: {
        fetch: async () =>
          ++round === 1
            ? callsResponse([{ id: 'a', value: variant === 'schema' ? 4 : 'x' }])
            : textResponse('openai-chat'),
      },
      validateArguments: schemaArguments,
      tools: {
        authorize() {
          authorizations++
          return variant === 'approval' ? 'ask' : 'auto'
        },
        async execute() {
          executions++
          return { kind: 'ok', text: 'not called' }
        },
      },
      approver: { wait: async () => false },
    })
    assert.equal(
      (await runner.run(run, new AbortController().signal, (event) => events.push(event))).status,
      'completed',
    )
    assert.equal(executions, 0)
    if (variant === 'catalogue') assert.equal(authorizations, 0)
    assert.equal(events.filter((event) => event.kind === 'approval_required').length, variant === 'approval' ? 1 : 0)
  }
})
test('approval allow executes once and plan hides edit tools', async () => {
  let calls = 0,
    executions = 0,
    approvals = 0
  const run = input()
  run.tools = [{ ...tool, approval: 'ask' }]
  const runner = engine(
    { fetch: async () => (++calls === 1 ? callsResponse([{ id: 'a', value: 'x' }]) : textResponse('openai-chat')) },
    async () => {
      executions++
      return { kind: 'ok', text: 'ok' }
    },
    'ask',
    async () => {
      approvals++
      return true
    },
  )
  assert.equal((await runner.run(run, new AbortController().signal, () => {})).status, 'completed')
  assert.equal(approvals, 1)
  assert.equal(executions, 1)
  run.permissionMode = 'plan'
  run.tools = [{ ...tool, effect: 'edit' }]
  assert.deepEqual(runner.validate(run).tools, [])
})
test('same call ID reuses unknown result; changed arguments fail without replay', async () => {
  for (const conflict of [false, true]) {
    let round = 0,
      executions = 0
    const run = input()
    run.tools = [tool]
    const runner = engine(
      {
        fetch: async () =>
          ++round <= 2
            ? callsResponse([{ id: 'a', value: conflict && round === 2 ? 'changed' : 'x' }])
            : textResponse('openai-chat'),
      },
      async () => {
        executions++
        return { kind: 'unknown', reason: 'disconnected' }
      },
    )
    const summary = await runner.run(run, new AbortController().signal, () => {})
    assert.equal(executions, 1)
    assert.equal(summary.status, conflict ? 'failed' : 'completed')
    if (conflict) assert.equal(summary.error?.details?.reason, 'tool_call_id_conflict')
    else
      assert.equal(
        summary.messages.filter((message) => 'tool' in message && message.tool.text.includes('不要自动重试')).length,
        2,
      )
  }
})
test('truncated tool round executes none; final round is exhausted completed', async () => {
  let executions = 0
  const run = input()
  run.tools = [tool]
  run.budget.maxRounds = 1
  const summary = await engine({ fetch: async () => callsResponse([{ id: 'a', value: 'x' }], true) }, async () => {
    executions++
    return { kind: 'ok', text: 'no' }
  }).run(run, new AbortController().signal, () => {})
  assert.equal(executions, 0)
  assert.equal(summary.status, 'completed')
  assert.equal(summary.exhausted, true)
})
test('only explicit pre-output 401/403/429 falls back; partial output never replays', async () => {
  for (const status of [401, 403, 429, 500]) {
    let requests = 0
    const summary = await engine({
      fetch: async () => (++requests === 1 ? new Response('secret key-one', { status }) : textResponse('openai-chat')),
    }).run(input(), new AbortController().signal, () => {})
    assert.equal(requests, status === 500 ? 1 : 2)
    assert.equal(summary.status, status === 500 ? 'failed' : 'completed')
    assert.ok(!summary.error?.message.includes('key-one'))
  }
  let requests = 0
  const summary = await engine({
    fetch: async () => {
      requests++
      return sse([{ choices: [{ delta: { content: 'partial' } }] }, { error: { message: 'stream broke key-one' } }])
    },
  }).run(input(), new AbortController().signal, () => {})
  assert.equal(requests, 1)
  assert.equal(summary.status, 'failed')
  assert.equal(summary.error?.code, 'upstream')
  assert.ok(!summary.error?.message.includes('key-one'))
  assert.ok(summary.messages.some((message) => 'assistant' in message && message.assistant.text === 'partial'))
})
test('dispatched cancellation confirms unknown and never starts another request', async () => {
  let requests = 0
  const abort = new AbortController(),
    events: RunEvent[] = []
  const run = input()
  run.tools = [tool]
  const runner = engine(
    {
      fetch: async () => {
        requests++
        return callsResponse([{ id: 'a', value: 'x' }])
      },
    },
    async () => {
      abort.abort()
      return { kind: 'unknown', reason: 'cancelled after dispatch' }
    },
  )
  const summary = await runner.run(run, abort.signal, (event) => events.push(event))
  assert.equal(summary.status, 'cancelled')
  assert.equal(requests, 1)
  assert.ok(events.some((event) => event.kind === 'tool_result' && event.outcome === 'unknown'))
  assert.equal(events.at(-1)?.kind, 'end')
})
test('SSE accepts all UTF-8 and CRLF splits and rejects unbounded input', () => {
  const bytes = new TextEncoder().encode('event: text\r\ndata: 你好\r\ndata: world\r\n\r\n')
  for (let split = 0; split <= bytes.length; split++) {
    const parser = new SseParser()
    assert.deepEqual(
      [...parser.feed(bytes.subarray(0, split)), ...parser.feed(bytes.subarray(split)), ...parser.finish()],
      [{ event: 'text', data: '你好\nworld' }],
    )
  }
  assert.throws(() => new SseParser(8, 16).feed(new TextEncoder().encode('data: 123456789')))
})
test('protocols encode current and historical images without replaying mismatched reasoning', () => {
  for (const protocol of ['anthropic-messages', 'openai-chat', 'openai-responses'] as const) {
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
              { protocol, endpointId: 'different', modelId: 'model', value: { type: 'thinking', thinking: 'secret' } },
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
  const usage = { ...emptyUsage(), inputTokens: 100, outputTokens: 20, cacheReadTokens: 40 }
  assert.deepEqual(
    cost({ currency: 'USD', tiers: [{ minInputTokens: 0, input: 2, output: 4, cacheRead: 1, cacheWrite: null }] }, [
      usage,
    ]),
    { currency: 'USD', amount: 0.00024 },
  )
  assert.equal(cost(null, [usage]), null)
  assert.equal(within('C:\\Safe\\Child\\..\\file', ['c:\\safe']), true)
  assert.equal(within('/safe/../unsafe', ['/safe']), false)
  assert.equal(within('relative', ['/']), false)
  assert.equal(shellRisk('sudo rm -rf /tmp/x')?.reason, 'elevation')
})
test('unknown tool never reaches executor and confirmed tool error keeps payload', async () => {
  for (const known of [false, true]) {
    let round = 0,
      executions = 0
    const run = input()
    run.tools = known ? [tool] : []
    const events: RunEvent[] = []
    const summary = await engine(
      { fetch: async () => (++round === 1 ? callsResponse([{ id: 'a', value: 'x' }]) : textResponse('openai-chat')) },
      async () => {
        executions++
        return {
          kind: 'error',
          code: 'read_failed',
          message: 'Not readable',
          payload: { detail: 'allowed functional data' },
        }
      },
    ).run(run, new AbortController().signal, (event) => events.push(event))
    assert.equal(summary.status, 'completed')
    assert.equal(executions, known ? 1 : 0)
    const result = events.find((event) => event.kind === 'tool_result')
    assert.equal(result?.kind, 'tool_result')
    if (result?.kind === 'tool_result') {
      assert.equal(result.outcome, 'error')
      if (known) assert.deepEqual(result.payload, { detail: 'allowed functional data' })
      else assert.ok(result.summary.includes('unknown_tool'))
    }
  }
})
test('cancellation while approval waits emits no execution and only one cancelled End', async () => {
  const abort = new AbortController(),
    events: RunEvent[] = []
  let executions = 0
  const run = input()
  run.tools = [{ ...tool, approval: 'ask' }]
  const runner = engine(
    { fetch: async () => callsResponse([{ id: 'a', value: 'x' }]) },
    async () => {
      executions++
      return { kind: 'ok', text: 'no' }
    },
    'ask',
    async () => {
      abort.abort()
      return false
    },
  )
  const result = await runner.run(run, abort.signal, (event) => events.push(event))
  assert.equal(result.status, 'cancelled')
  assert.equal(executions, 0)
  assert.equal(events.filter((event) => event.kind === 'end').length, 1)
})
test('sink failure cancels before dispatch and invalid direct input fails with one End', async () => {
  let requests = 0
  const runner = engine({
    fetch: async () => {
      requests++
      return textResponse('openai-chat')
    },
  })
  const result = await runner.run(input(), new AbortController().signal, () => {
    throw new Error('closed')
  })
  assert.equal(result.status, 'cancelled')
  assert.equal(requests, 0)
  const invalid = input()
  invalid.tools = [tool, tool]
  const events: RunEvent[] = []
  const failed = await runner.run(invalid, new AbortController().signal, (event) => events.push(event))
  assert.equal(failed.status, 'failed')
  assert.equal(requests, 0)
  assert.equal(events.filter((event) => event.kind === 'end').length, 1)
})
