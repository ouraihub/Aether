import type { Protocol } from '@ouraihub/aether-contracts'
import { RunEngine } from '../src/engine/index.js'
import type { RunInput, ToolExecution, ToolSpec } from '../src/types.js'
import { DEFAULT_BUDGET } from '../src/types.js'
import type { ArgumentsValidator, OutboundHttp } from '../src/ports.js'
export const schemaArguments: ArgumentsValidator = (schema, args) =>
  schema.type === 'object' &&
  typeof args.value === 'string' &&
  Object.keys(args).every((key) => key === 'value')
export const tool: ToolSpec = {
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
export function input(protocol: Protocol = 'openai-chat'): RunInput {
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
export function sse(events: readonly unknown[]): Response {
  const encoded = new TextEncoder().encode(
    events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(''),
  )
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        for (let i = 0; i < encoded.length; i += 7)
          controller.enqueue(encoded.subarray(i, i + 7))
        controller.close()
      },
    }),
    { headers: { 'content-type': 'text/event-stream' } },
  )
}
export function textResponse(protocol: Protocol): Response {
  if (protocol === 'anthropic-messages')
    return sse([
      {
        type: 'message_start',
        message: {
          usage: {
            input_tokens: 10,
            cache_read_input_tokens: 3,
            cache_creation_input_tokens: 2,
          },
        },
      },
      {
        type: 'content_block_delta',
        index: 0,
        delta: { type: 'text_delta', text: '你好' },
      },
      {
        type: 'message_delta',
        delta: { stop_reason: 'end_turn' },
        usage: { output_tokens: 4 },
      },
      { type: 'message_stop' },
    ])
  if (protocol === 'openai-responses')
    return sse([
      { type: 'response.output_text.delta', delta: '你好' },
      {
        type: 'response.completed',
        response: { usage: { input_tokens: 15, output_tokens: 4 } },
      },
    ])
  return sse([
    { choices: [{ delta: { content: '你好' }, finish_reason: null }] },
    {
      choices: [{ delta: {}, finish_reason: 'stop' }],
      usage: { prompt_tokens: 15, completion_tokens: 4 },
    },
  ])
}
export function callsResponse(
  calls: readonly { id: string; value: unknown }[],
  truncated = false,
): Response {
  return sse([
    {
      choices: [
        {
          delta: {
            tool_calls: calls.map((call, index) => ({
              index,
              id: call.id,
              function: {
                name: tool.name,
                arguments: JSON.stringify({ value: call.value }),
              },
            })),
          },
          finish_reason: null,
        },
      ],
    },
    {
      choices: [
        { delta: {}, finish_reason: truncated ? 'length' : 'tool_calls' },
      ],
      usage: { prompt_tokens: 12, completion_tokens: 2 },
    },
  ])
}
export function engine(
  outbound: OutboundHttp,
  execute: () => Promise<ToolExecution> = async () => ({
    kind: 'ok',
    text: 'value',
  }),
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
