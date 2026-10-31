import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import type { RunEvent } from '@ouraihub/aether-contracts'
import { RunEngine } from '../src/engine/index.js'
import {
  schemaArguments,
  tool,
  input,
  textResponse,
  callsResponse,
  engine,
  sse,
} from './engine-support.js'

test('terminal text-only token limits preserve the reply without replaying a model request', async () => {
  for (const protocol of [
    'anthropic-messages',
    'openai-chat',
    'openai-responses',
  ] as const) {
    const bytes = await readFile(
      new URL(
        `../../ui/fixtures/sse/${protocol}/truncated.sse`,
        import.meta.url,
      ),
    )
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
    const result = await engine.run(
      input(protocol),
      new AbortController().signal,
      (event) => events.push(event),
    )
    assert.equal(result.status, 'completed')
    assert.equal(requests, 1)
    assert.equal(events.filter((event) => event.kind === 'end').length, 1)
    assert.ok(
      result.messages.some(
        (message) => 'assistant' in message && message.assistant.text,
      ),
    )
  }
})
for (const protocol of [
  'openai-chat',
  'openai-responses',
  'anthropic-messages',
] as const) {
  test(`${protocol}: real chunked stream, usage and one final End`, async () => {
    const events: RunEvent[] = [],
      requests: RequestInit[] = []
    const summary = await engine({
      fetch: async (_url, init) => {
        requests.push(init)
        return textResponse(protocol)
      },
    }).run(input(protocol), new AbortController().signal, (event) =>
      events.push(event),
    )
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
    run.tools = [
      { ...tool, approval: variant === 'catalogue' ? 'deny' : 'ask' },
    ]
    const events: RunEvent[] = []
    const runner = new RunEngine({
      outbound: {
        fetch: async () =>
          ++round === 1
            ? callsResponse([
                { id: 'a', value: variant === 'schema' ? 4 : 'x' },
              ])
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
      (
        await runner.run(run, new AbortController().signal, (event) =>
          events.push(event),
        )
      ).status,
      'completed',
    )
    assert.equal(executions, 0)
    if (variant === 'catalogue') assert.equal(authorizations, 0)
    assert.equal(
      events.filter((event) => event.kind === 'approval_required').length,
      variant === 'approval' ? 1 : 0,
    )
  }
})
test('approval allow executes once and plan hides edit tools', async () => {
  let calls = 0,
    executions = 0,
    approvals = 0
  const run = input()
  run.tools = [{ ...tool, approval: 'ask' }]
  const runner = engine(
    {
      fetch: async () =>
        ++calls === 1
          ? callsResponse([{ id: 'a', value: 'x' }])
          : textResponse('openai-chat'),
    },
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
  assert.equal(
    (await runner.run(run, new AbortController().signal, () => {})).status,
    'completed',
  )
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
            ? callsResponse([
                { id: 'a', value: conflict && round === 2 ? 'changed' : 'x' },
              ])
            : textResponse('openai-chat'),
      },
      async () => {
        executions++
        return { kind: 'unknown', reason: 'disconnected' }
      },
    )
    const summary = await runner.run(
      run,
      new AbortController().signal,
      () => {},
    )
    assert.equal(executions, 1)
    assert.equal(summary.status, conflict ? 'failed' : 'completed')
    if (conflict)
      assert.equal(summary.error?.details?.reason, 'tool_call_id_conflict')
    else
      assert.equal(
        summary.messages.filter(
          (message) =>
            'tool' in message && message.tool.text.includes('不要自动重试'),
        ).length,
        2,
      )
  }
})
test('truncated tool round executes none; final round is exhausted completed', async () => {
  let executions = 0
  const run = input()
  run.tools = [tool]
  run.budget.maxRounds = 1
  const summary = await engine(
    { fetch: async () => callsResponse([{ id: 'a', value: 'x' }], true) },
    async () => {
      executions++
      return { kind: 'ok', text: 'no' }
    },
  ).run(run, new AbortController().signal, () => {})
  assert.equal(executions, 0)
  assert.equal(summary.status, 'completed')
  assert.equal(summary.exhausted, true)
})
test('only explicit pre-output 401/403/429 falls back; partial output never replays', async () => {
  for (const status of [401, 403, 429, 500]) {
    let requests = 0
    const summary = await engine({
      fetch: async () =>
        ++requests === 1
          ? new Response('secret key-one', { status })
          : textResponse('openai-chat'),
    }).run(input(), new AbortController().signal, () => {})
    assert.equal(requests, status === 500 ? 1 : 2)
    assert.equal(summary.status, status === 500 ? 'failed' : 'completed')
    assert.ok(!summary.error?.message.includes('key-one'))
  }
  let requests = 0
  const summary = await engine({
    fetch: async () => {
      requests++
      return sse([
        { choices: [{ delta: { content: 'partial' } }] },
        { error: { message: 'stream broke key-one' } },
      ])
    },
  }).run(input(), new AbortController().signal, () => {})
  assert.equal(requests, 1)
  assert.equal(summary.status, 'failed')
  assert.equal(summary.error?.code, 'upstream')
  assert.ok(!summary.error?.message.includes('key-one'))
  assert.ok(
    summary.messages.some(
      (message) =>
        'assistant' in message && message.assistant.text === 'partial',
    ),
  )
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
  const summary = await runner.run(run, abort.signal, (event) =>
    events.push(event),
  )
  assert.equal(summary.status, 'cancelled')
  assert.equal(requests, 1)
  assert.ok(
    events.some(
      (event) => event.kind === 'tool_result' && event.outcome === 'unknown',
    ),
  )
  assert.equal(events.at(-1)?.kind, 'end')
})
test('unknown tool never reaches executor and confirmed tool error keeps payload', async () => {
  for (const known of [false, true]) {
    let round = 0,
      executions = 0
    const run = input()
    run.tools = known ? [tool] : []
    const events: RunEvent[] = []
    const summary = await engine(
      {
        fetch: async () =>
          ++round === 1
            ? callsResponse([{ id: 'a', value: 'x' }])
            : textResponse('openai-chat'),
      },
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
      if (known)
        assert.deepEqual(result.payload, { detail: 'allowed functional data' })
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
  const result = await runner.run(run, abort.signal, (event) =>
    events.push(event),
  )
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
  const failed = await runner.run(
    invalid,
    new AbortController().signal,
    (event) => events.push(event),
  )
  assert.equal(failed.status, 'failed')
  assert.equal(requests, 0)
  assert.equal(events.filter((event) => event.kind === 'end').length, 1)
})
