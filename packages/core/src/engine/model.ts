import type { TokenUsage } from '@ouraihub/aether-contracts'
import type { EventSink, OutboundHttp } from '../ports.js'
import type { Message, ReasoningBlock, RunInput, ToolCall } from '../types.js'
import { cancelled, excerpt, KitFailure } from '../error.js'
import { charCount } from '../guard.js'
import { SseParser } from '../sse.js'
import { Decoder } from '../protocol/decoder.js'
import { authHeaders, PROTOCOL_PATH, requestBody } from '../protocol/request.js'
import { reasoningBudget } from './prepare.js'

export interface RoundState {
  text: string
  reasoning: ReasoningBlock[]
  requestChars: number
}
export interface RoundResult {
  calls: ToolCall[]
  truncated: boolean
}
export async function boundedWait<T>(
  promise: Promise<T>,
  signal: AbortSignal,
  ms: number,
  abort: () => void,
  reason: string,
): Promise<T> {
  if (signal.aborted) {
    abort()
    throw cancelled()
  }
  const stop = Promise.withResolvers<never>()
  const cancel = (): void => {
    abort()
    stop.reject(cancelled())
  }
  signal.addEventListener('abort', cancel, { once: true })
  const timer = setTimeout(() => {
    abort()
    stop.reject(new KitFailure('timeout', 'Execution timeout', { reason }))
  }, ms)
  try {
    return await Promise.race([promise, stop.promise])
  } finally {
    clearTimeout(timer)
    signal.removeEventListener('abort', cancel)
  }
}
async function errorBody(
  response: Response,
  signal: AbortSignal,
  abort: () => void,
): Promise<string> {
  if (!response.body) return ''
  const reader = response.body.getReader(),
    chunks: Uint8Array[] = []
  let length = 0
  try {
    while (length < 8192) {
      const next = await boundedWait(
        reader.read(),
        signal,
        15000,
        abort,
        'error_body',
      )
      if (next.done) break
      const chunk = next.value.subarray(0, 8192 - length)
      chunks.push(chunk)
      length += chunk.length
    }
  } finally {
    void reader.cancel().catch(() => {})
  }
  const bytes = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.length
  }
  return new TextDecoder().decode(bytes)
}
export async function streamRound(
  outbound: OutboundHttp,
  input: RunInput,
  system: string,
  turn: readonly Message[],
  signal: AbortSignal,
  usage: TokenUsage,
  state: RoundState,
  emit: EventSink,
): Promise<RoundResult> {
  if (signal.aborted) throw cancelled()
  const connection = input.connection
  const encoded = requestBody({
    protocol: connection.protocol,
    endpointId: connection.endpointId,
    model: connection.modelId,
    baseUrl: connection.baseUrl,
    system,
    history: input.history,
    input: input.input,
    images: input.images,
    maxOutput: input.budget.maxOutput,
    reasoningEffort: input.reasoningEffort,
    reasoning: connection.reasoning,
    reasoningBudget: reasoningBudget(input),
    nativeWebSearch: input.nativeWebSearch,
    tools: input.tools,
    turn,
  })
  const body = JSON.stringify(encoded)
  state.requestChars = charCount(
    JSON.stringify(encoded, (key, value: unknown) =>
      key === 'images' ||
      key === 'image_url' ||
      key === 'data' ||
      key === 'source'
        ? undefined
        : value,
    ),
  )
  const keys: readonly (string | null)[] = connection.keys.length
    ? connection.keys
    : [null]
  for (let index = 0; index < keys.length; index++) {
    const controller = new AbortController()
    const abort = (): void => controller.abort()
    const cancel = (): void => controller.abort()
    signal.addEventListener('abort', cancel, { once: true })
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined
    const decoder = new Decoder(
      connection.protocol,
      connection.endpointId,
      connection.modelId,
    )
    try {
      if (signal.aborted) throw cancelled()
      const headers = new Headers(connection.headers)
      headers.set('accept', 'text/event-stream')
      headers.set('content-type', 'application/json')
      for (const [name, value] of Object.entries(
        authHeaders(connection.protocol, keys[index] ?? null),
      ))
        headers.set(name, value)
      const response = await boundedWait(
        outbound.fetch(
          `${connection.baseUrl.replace(/\/+$/, '')}/${PROTOCOL_PATH[connection.protocol]}`,
          {
            method: 'POST',
            headers,
            body,
            redirect: 'manual',
            signal: controller.signal,
          },
        ),
        signal,
        15000,
        abort,
        'connect',
      )
      if (!response.ok) {
        const raw = await errorBody(response, signal, abort)
        if (
          [401, 403, 429].includes(response.status) &&
          index + 1 < keys.length
        )
          continue
        const code =
          response.status === 401 || response.status === 403
            ? 'unauthorized'
            : response.status === 429
              ? 'rate_limited'
              : 'upstream'
        const unsupportedTools =
          response.status === 400 &&
          /(?:tools?|function.calling).{0,80}(?:not supported|unsupported|not available)|(?:not supported|unsupported).{0,80}(?:tools?|function.calling)/is.test(
            raw,
          )
        throw new KitFailure(
          code,
          `Upstream ${response.status}: ${excerpt(raw, [...connection.keys, ...Object.values(connection.headers)])}`,
          {
            status: response.status,
            ...(unsupportedTools ? { reason: 'tools_unsupported' } : {}),
          },
        )
      }
      if (!response.body)
        throw new KitFailure('network', 'Empty upstream stream')
      reader = response.body.getReader()
      const parser = new SseParser()
      const apply = (data: string): void => {
        const event = decoder.event(data, usage)
        if (!event) return
        if (event.error !== undefined)
          throw new KitFailure(
            'upstream',
            excerpt(event.error, [
              ...connection.keys,
              ...Object.values(connection.headers),
            ]),
            { reason: 'stream_error' },
          )
        if (event.reasoning)
          emit({ kind: 'reasoning_delta', text: event.reasoning })
        if (event.text) {
          state.text += event.text
          emit({ kind: 'text_delta', text: event.text })
        }
      }
      let first = true
      for (;;) {
        const next = await boundedWait(
          reader.read(),
          signal,
          first ? 120000 : 60000,
          abort,
          first ? 'first_byte' : 'idle',
        )
        if (next.done) break
        first = false
        for (const event of parser.feed(next.value)) apply(event.data)
      }
      for (const event of parser.finish()) apply(event.data)
      if (!decoder.done)
        throw new KitFailure('network', 'Upstream stream ended early', {
          reason: 'incomplete_stream',
        })
      const decoded = decoder.toolCalls(),
        calls: ToolCall[] = []
      let truncated = decoder.truncated && decoded.length > 0
      for (const call of decoded) {
        if (call.args === null || !call.id || !call.name) truncated = true
        else calls.push({ id: call.id, name: call.name, args: call.args })
      }
      return { calls, truncated }
    } catch (error) {
      if (signal.aborted) throw cancelled()
      if (error instanceof KitFailure) throw error
      throw new KitFailure('network', 'Upstream connection failed')
    } finally {
      decoder.finishReasoning()
      state.reasoning = decoder.reasoning
      if (reader) void reader.cancel().catch(() => {})
      controller.abort()
      signal.removeEventListener('abort', cancel)
    }
  }
  throw new KitFailure('unauthorized', 'No usable API key')
}
