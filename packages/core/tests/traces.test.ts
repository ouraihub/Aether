// Golden traces of the public run behaviour (docs/12 §5): every event, every request body the
// upstream received, what the host saw and what the summary feeds back, per scenario and protocol.
// Any change to the engine's observable behaviour changes `traces.json`; refactors must not.
// Regenerate deliberately with `AETHER_WRITE_TRACES=1 pnpm --filter @ouraihub/aether-core test`.
import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, writeFileSync } from 'node:fs'
import type { Approval, PermissionMode, Protocol, RunEvent, ToolEffect } from '@ouraihub/aether-contracts'
import { RunEngine } from '../src/engine/index.js'
import { validateArguments } from '../src/schema.js'
import type { RunInput, ToolExecution, ToolSource, ToolSpec } from '../src/types.js'
import { DEFAULT_BUDGET } from '../src/types.js'

const GOLDEN = new URL('./traces.json', import.meta.url)
const PROTOCOLS: readonly Protocol[] = ['anthropic-messages', 'openai-chat', 'openai-responses']

function fixture(protocol: Protocol, name: string): string {
  return readFileSync(new URL(`../../ui/fixtures/sse/${protocol}/${name}.sse`, import.meta.url), 'utf8')
}
function edit(body: string, from: string, to: string): string {
  assert.ok(body.includes(from), from)
  return body.replace(from, to)
}
/** `single-tool` with complete arguments but stopped by the output limit. */
function limited(protocol: Protocol): string {
  const single = fixture(protocol, 'single-tool')
  if (protocol === 'anthropic-messages') return edit(single, '"stop_reason":"tool_use"', '"stop_reason":"max_tokens"')
  if (protocol === 'openai-chat') return edit(single, '"finish_reason":"tool_calls"', '"finish_reason":"length"')
  return edit(
    edit(single, '"type":"response.completed"', '"type":"response.incomplete"'),
    '"incomplete_details":null,"usage":{',
    '"incomplete_details":{"reason":"max_output_tokens"},"usage":{',
  )
}

/** One upstream answer: a full SSE body, one cut after its first text delta, or a status. */
type Reply = { sse: string } | { cut: string } | { status: number; body: string }
const sse = (body: string): Reply => ({ sse: body })

function respond(reply: Reply): Response {
  if ('status' in reply) return new Response(reply.body, { status: reply.status })
  const headers = { 'content-type': 'text/event-stream' }
  if ('sse' in reply) return new Response(reply.sse, { headers })
  // Up to the end of the first text delta, then silence: the run's cancel must end the read.
  const marker = /(text_delta|output_text\.delta|"content":"[^"])/.exec(reply.cut)
  assert.ok(marker)
  const end = reply.cut.indexOf('\n\n', marker.index) + 2
  const bytes = new TextEncoder().encode(reply.cut.slice(0, end))
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes)
      },
    }),
    { headers },
  )
}

interface Host {
  /** `null` passes the proposed approval through. */
  approval: Approval | null
  /** `null` waits until the run is cancelled. */
  allow: boolean | null
  reply: ToolExecution
  cancelInExecute: boolean
}
interface Scenario {
  name: string
  replies: (protocol: Protocol) => Reply[]
  tools: () => ToolSpec[]
  host: Host
  mode: PermissionMode
  maxRounds: number | null
  cancel: 'never' | 'approval' | 'first_text'
}

function tool(approval: Approval, effect: ToolEffect, source: ToolSource): ToolSpec {
  return { name: 'read_file', description: 'read_file tool', parameters: { type: 'object' }, approval, effect, source }
}
const autoRead = (): ToolSpec[] => [tool('auto', 'read', { kind: 'host' })]
const askRead = (): ToolSpec[] => [tool('ask', 'read', { kind: 'host' })]
const multiThenText = (p: Protocol): Reply[] => [sse(fixture(p, 'multi-tool')), sse(fixture(p, 'text'))]
const base = (
  name: string,
  replies: Scenario['replies'],
  tools: Scenario['tools'],
  host: Partial<Host> = {},
): Scenario => ({
  name,
  replies,
  tools,
  host: { approval: null, allow: true, reply: { kind: 'ok', text: 'contents' }, cancelInExecute: false, ...host },
  mode: 'default',
  maxRounds: null,
  cancel: 'never',
})

const scenarios: Scenario[] = [
  base('text', (p) => [sse(fixture(p, 'text'))], autoRead),
  base('auto_two_tools', multiThenText, autoRead),
  base('ask_allow', multiThenText, askRead),
  base('ask_deny', multiThenText, askRead, { allow: false }),
  base('unknown_result', multiThenText, autoRead, { reply: { kind: 'unknown', reason: 'timeout' } }),
  base('error_with_payload', multiThenText, autoRead, {
    reply: { kind: 'error', code: 'invalid', message: 'fix arguments', payload: { diagnostic: 'ui only' } },
  }),
  base(
    'repeated_call_ids',
    (p) => [sse(fixture(p, 'multi-tool')), sse(fixture(p, 'multi-tool')), sse(fixture(p, 'text'))],
    autoRead,
  ),
  base(
    'reused_id_changed_args',
    (p) => [sse(fixture(p, 'multi-tool')), sse(fixture(p, 'multi-tool').replace('a.md', 'changed.md'))],
    autoRead,
  ),
  base('truncated_round', (p) => [sse(limited(p)), sse(fixture(p, 'text'))], autoRead),
  {
    ...base('round_limit', (p) => [sse(fixture(p, 'multi-tool')), sse(fixture(p, 'multi-tool'))], autoRead),
    maxRounds: 2,
  },
  { ...base('cancel_at_approval', multiThenText, askRead, { allow: null }), cancel: 'approval' },
  base('cancel_in_tool', multiThenText, autoRead, { cancelInExecute: true }),
  { ...base('cancel_mid_stream', (p) => [{ cut: fixture(p, 'text') }], autoRead), cancel: 'first_text' },
  base('in_stream_error', (p) => [sse(fixture(p, 'upstream-error'))], autoRead),
  base('status_500', () => [{ status: 500, body: '{"error":{"message":"boom"}}' }], autoRead),
  base(
    'tools_rejected_retry',
    (p) => [{ status: 400, body: '{"error":{"message":"tools are not supported"}}' }, sse(fixture(p, 'text'))],
    autoRead,
  ),
  { ...base('plan_hides_edit_tool', multiThenText, () => [tool('auto', 'edit', { kind: 'host' })]), mode: 'plan' },
  base('catalogue_deny', multiThenText, () => [tool('deny', 'read', { kind: 'host' })], { approval: 'auto' }),
  {
    ...base('mcp_tool_full_access', multiThenText, () => [tool('ask', 'other', { kind: 'mcp', server: 'docs' })]),
    mode: 'full_access',
  },
]

function input(scenario: Scenario, protocol: Protocol): RunInput {
  return {
    runId: 'run',
    purpose: 'notes',
    permissionMode: scenario.mode,
    connection: {
      protocol,
      endpointId: 'endpoint',
      modelId: 'model',
      baseUrl: 'https://models.example/v1',
      keys: ['sk-trace-0123456789'],
      headers: {},
      supportsTools: null,
      canVision: false,
      maxOutput: null,
      contextWindow: null,
      reasoning: null,
      pricing: null,
    },
    system: [{ kind: 'host', text: 'be brief' }],
    history: [],
    input: 'hi',
    images: [],
    tools: scenario.tools(),
    budget: { ...DEFAULT_BUDGET, ...(scenario.maxRounds === null ? {} : { maxRounds: scenario.maxRounds }) },
    reasoningEffort: 'default',
    nativeWebSearch: false,
  }
}

async function trace(scenario: Scenario, protocol: Protocol): Promise<unknown> {
  const replies = scenario.replies(protocol)
  const requests: unknown[] = [],
    host: unknown[] = [],
    events: RunEvent[] = []
  const cancel = new AbortController()
  const engine = new RunEngine({
    outbound: {
      fetch: async (url, init) => {
        requests.push({ url, body: JSON.parse(String(init.body)) })
        const reply = replies.shift() ?? { status: 599, body: 'no scripted reply' }
        return respond(reply)
      },
    },
    tools: {
      authorize: (call, context) => {
        host.push({ authorize: call.id, proposed: context.proposed })
        return scenario.host.approval ?? context.proposed
      },
      execute: async (call) => {
        host.push({ execute: call.id })
        if (scenario.host.cancelInExecute) {
          cancel.abort()
          return { kind: 'unknown', reason: 'cancelled' }
        }
        return scenario.host.reply
      },
    },
    approver: {
      wait: async (_run, call, signal) => {
        host.push({ wait: call.id })
        if (scenario.host.allow !== null) return scenario.host.allow
        const aborted = Promise.withResolvers<void>()
        signal.addEventListener('abort', () => aborted.resolve(), { once: true })
        await aborted.promise
        return false
      },
    },
    validateArguments,
  })
  const summary = await engine.run(input(scenario, protocol), cancel.signal, (event) => {
    events.push(event)
    if (
      (scenario.cancel === 'approval' && event.kind === 'approval_required') ||
      (scenario.cancel === 'first_text' && event.kind === 'text_delta')
    )
      cancel.abort()
  })
  return normalize({
    events,
    host,
    requests,
    summary: { status: summary.status, exhausted: summary.exhausted, messages: summary.messages, error: summary.error },
  })
}

/** Replaces the per-run random tool-result boundary so traces compare exactly. */
function normalize(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value).replace(/TOOL_RESULT_[0-9a-f]{12}/g, 'TOOL_RESULT_<boundary>'))
}

test('public run behaviour matches the golden traces', async () => {
  const actual: Record<string, unknown> = {}
  for (const scenario of scenarios)
    for (const protocol of PROTOCOLS) actual[`${scenario.name}/${protocol}`] = await trace(scenario, protocol)
  if (process.env.AETHER_WRITE_TRACES) {
    writeFileSync(GOLDEN, JSON.stringify(actual, null, 2) + '\n')
    return
  }
  const golden = JSON.parse(readFileSync(GOLDEN, 'utf8')) as Record<string, unknown>
  for (const [key, want] of Object.entries(golden)) assert.deepEqual(actual[key], want, `trace ${key} changed`)
  assert.deepEqual(Object.keys(actual).sort(), Object.keys(golden).sort())
})
