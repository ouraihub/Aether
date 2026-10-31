import type { Protocol, TokenUsage } from '@ouraihub/aether-contracts'
import type { ReasoningBlock } from '../types.js'
import { at, isRecord } from '../guard.js'
import { KitFailure } from '../error.js'
type Usage = TokenUsage
export type Decoded = {
  text?: string
  reasoning?: string
  error?: string
} | null

const tokens = (value: unknown): number | null =>
  typeof value === 'number' &&
  Number.isInteger(value) &&
  value >= 0 &&
  value <= 0xffff_ffff
    ? value
    : null

const text = (value: unknown): Decoded =>
  typeof value === 'string' && value ? { text: value } : null

const error = (value: unknown): Decoded => {
  const message = at(value, 'message')
  return { error: typeof message === 'string' ? message : 'upstream error' }
}

function citation(value: unknown): Decoded {
  const raw = at(value, 'url')
  if (typeof raw !== 'string') return null
  try {
    const url = new URL(raw)
    if (url.protocol !== 'https:' && url.protocol !== 'http:') return null
    return {
      text: ` [source](<${url.href.replaceAll('<', '%3C').replaceAll('>', '%3E')}>)`,
    }
  } catch {
    return null
  }
}

export interface DecodedToolCall {
  id: string
  name: string
  args: Record<string, unknown> | null
}

export class Decoder {
  done = false
  truncated = false
  readonly reasoning: ReasoningBlock[] = []
  private blocks = new Map<number, Record<string, unknown>>()
  private baseInput: number | null = null
  private chatReasoning = ''
  /** Streaming calls; key is the anthropic block index, openai-chat `index`, or responses `item_id`. */
  private calls: { key: unknown; id: string; name: string; args: string }[] = []
  private readonly protocol: Protocol
  private readonly endpointId: string | null
  private readonly modelId: string | null
  constructor(
    protocol: Protocol,
    endpointId: string | null = null,
    modelId: string | null = null,
  ) {
    this.protocol = protocol
    this.endpointId = endpointId
    this.modelId = modelId
  }
  finishReasoning(): void {
    if (this.chatReasoning) {
      this.keep(this.chatReasoning)
      this.chatReasoning = ''
    }
  }

  private keep(value: unknown): void {
    const block = {
      protocol: this.protocol,
      endpointId: this.endpointId,
      modelId: this.modelId,
      value,
    }
    const id = at(value, 'id')
    const existing =
      id == null
        ? -1
        : this.reasoning.findIndex((item) => at(item.value, 'id') === id)
    if (existing < 0) this.reasoning.push(block)
    else this.reasoning[existing] = block
  }

  private start(key: unknown, id: unknown, name: unknown): void {
    if (this.calls.length >= 128)
      throw new KitFailure('upstream', 'Tool call limit exceeded')
    this.calls.push({
      key,
      id: typeof id === 'string' ? id : '',
      name: typeof name === 'string' ? name : '',
      args: '',
    })
  }

  private append(key: unknown, fragment: unknown): void {
    const call = this.calls.find((item) => item.key === key)
    if (call && typeof fragment === 'string') {
      if (call.args.length + fragment.length > 262144)
        throw new KitFailure('upstream', 'Tool arguments limit exceeded')
      call.args += fragment
    }
  }

  /** Calls in stream order; arguments that are not a JSON object come back as `null`. */
  toolCalls(): DecodedToolCall[] {
    return this.calls.map(({ id, name, args }) => {
      let parsed: unknown = null
      try {
        parsed = JSON.parse(args || '{}')
      } catch {
        /* stays null */
      }
      return { id, name, args: isRecord(parsed) ? parsed : null }
    })
  }

  event(data: string, usage: Usage): Decoded {
    if (data === '[DONE]') {
      this.done = true
      return null
    }
    let v: unknown
    try {
      v = JSON.parse(data)
    } catch {
      throw new KitFailure('upstream', 'Invalid SSE JSON')
    }
    const kind = at(v, 'type')
    if (this.protocol === 'anthropic-messages') {
      if (kind === 'message_start' || kind === 'message_delta') {
        const raw =
          kind === 'message_start' ? at(v, 'message', 'usage') : at(v, 'usage')
        this.baseInput = tokens(at(raw, 'input_tokens')) ?? this.baseInput
        usage.outputTokens =
          tokens(at(raw, 'output_tokens')) ?? usage.outputTokens
        usage.cacheReadTokens =
          tokens(at(raw, 'cache_read_input_tokens')) ?? usage.cacheReadTokens
        usage.cacheWriteTokens =
          tokens(at(raw, 'cache_creation_input_tokens')) ??
          usage.cacheWriteTokens
        usage.reasoningTokens =
          tokens(at(raw, 'reasoning_tokens')) ?? usage.reasoningTokens
        if (this.baseInput !== null)
          usage.inputTokens = Math.min(
            0xffff_ffff,
            this.baseInput +
              (usage.cacheReadTokens ?? 0) +
              (usage.cacheWriteTokens ?? 0),
          )
        if (at(v, 'delta', 'stop_reason') === 'max_tokens')
          this.truncated = true
      } else if (kind === 'content_block_start') {
        const block = at(v, 'content_block')
        if (
          at(block, 'type') === 'web_search_tool_result' &&
          at(block, 'content', 'type') === 'web_search_tool_result_error'
        )
          return { error: 'native web search failed' }
        const index = tokens(at(v, 'index'))
        if (at(block, 'type') === 'tool_use') {
          this.start(index, at(block, 'id'), at(block, 'name'))
          const input = at(block, 'input')
          if (isRecord(input) && Object.keys(input).length)
            this.append(index, JSON.stringify(input))
        }
        if (
          index !== null &&
          isRecord(block) &&
          (at(block, 'type') === 'thinking' ||
            at(block, 'type') === 'redacted_thinking')
        ) {
          this.blocks.set(index, { ...block })
          const initial = at(block, 'thinking')
          if (typeof initial === 'string' && initial)
            return { reasoning: initial }
        }
      } else if (kind === 'content_block_delta') {
        const delta = at(v, 'delta')
        if (at(delta, 'type') === 'text_delta') return text(at(delta, 'text'))
        if (at(delta, 'type') === 'citations_delta')
          return citation(at(delta, 'citation'))
        if (at(delta, 'type') === 'input_json_delta') {
          this.append(tokens(at(v, 'index')), at(delta, 'partial_json'))
          return null
        }
        const block = this.blocks.get(Number(at(v, 'index')))
        if (block && at(delta, 'type') === 'thinking_delta') {
          const fragment = at(delta, 'thinking')
          if (typeof fragment === 'string') {
            block.thinking = String(block.thinking ?? '') + fragment
            return { reasoning: fragment }
          }
        }
        if (block && at(delta, 'type') === 'signature_delta') {
          const fragment = at(delta, 'signature')
          if (typeof fragment === 'string')
            block.signature = String(block.signature ?? '') + fragment
        }
      } else if (kind === 'content_block_stop') {
        const index = Number(at(v, 'index')),
          block = this.blocks.get(index)
        if (
          block &&
          ((block.type === 'thinking' &&
            typeof block.signature === 'string' &&
            block.signature) ||
            (block.type === 'redacted_thinking' &&
              typeof block.data === 'string' &&
              block.data))
        )
          this.keep(block)
        this.blocks.delete(index)
      } else if (kind === 'message_stop') this.done = true
      else if (kind === 'error') return error(at(v, 'error'))
      return null
    }
    if (this.protocol === 'openai-chat') {
      if (at(v, 'error') != null) return error(at(v, 'error'))
      const raw = at(v, 'usage')
      if (raw && typeof raw === 'object') {
        usage.inputTokens =
          tokens(at(raw, 'prompt_tokens')) ?? usage.inputTokens
        usage.outputTokens =
          tokens(at(raw, 'completion_tokens')) ?? usage.outputTokens
        usage.cacheReadTokens =
          tokens(at(raw, 'prompt_tokens_details', 'cached_tokens')) ??
          usage.cacheReadTokens
        usage.cacheWriteTokens =
          tokens(at(raw, 'prompt_tokens_details', 'cache_write_tokens')) ??
          usage.cacheWriteTokens
        usage.reasoningTokens =
          tokens(at(raw, 'completion_tokens_details', 'reasoning_tokens')) ??
          usage.reasoningTokens
      }
      const finish = at(v, 'choices', 0, 'finish_reason')
      if (finish != null) {
        this.done = true
        this.truncated = finish === 'length'
      }
      const deltas = at(v, 'choices', 0, 'delta', 'tool_calls')
      if (Array.isArray(deltas))
        for (const delta of deltas) {
          const key = at(delta, 'index')
          if (!this.calls.some((item) => item.key === key))
            this.start(key, at(delta, 'id'), at(delta, 'function', 'name'))
          this.append(key, at(delta, 'function', 'arguments'))
        }
      const thinking = at(v, 'choices', 0, 'delta', 'reasoning_content')
      if (typeof thinking === 'string' && thinking)
        this.chatReasoning += thinking
      if (finish != null && this.chatReasoning) {
        this.keep(this.chatReasoning)
        this.chatReasoning = ''
      }
      const content = at(v, 'choices', 0, 'delta', 'content')
      if (typeof thinking === 'string' && thinking)
        return {
          reasoning: thinking,
          ...(typeof content === 'string' && content ? { text: content } : {}),
        }
      return text(at(v, 'choices', 0, 'delta', 'content'))
    }
    if (
      kind === 'response.output_item.done' &&
      at(v, 'item', 'type') === 'reasoning'
    )
      this.keep(at(v, 'item'))
    // `call_id` (not the item id) is what `function_call_output` must echo.
    if (
      kind === 'response.output_item.added' &&
      at(v, 'item', 'type') === 'function_call'
    )
      this.start(
        at(v, 'item', 'id'),
        at(v, 'item', 'call_id'),
        at(v, 'item', 'name'),
      )
    if (kind === 'response.function_call_arguments.delta')
      this.append(at(v, 'item_id'), at(v, 'delta'))
    if (kind === 'response.reasoning_summary_text.delta') {
      const fragment = at(v, 'delta')
      if (typeof fragment === 'string') return { reasoning: fragment }
    }
    if (kind === 'response.output_text.delta') return text(at(v, 'delta'))
    if (kind === 'response.output_text.annotation.added')
      return citation(at(v, 'annotation'))
    if (kind === 'response.completed' || kind === 'response.incomplete') {
      const raw = at(v, 'response', 'usage')
      usage.inputTokens = tokens(at(raw, 'input_tokens'))
      usage.outputTokens = tokens(at(raw, 'output_tokens'))
      usage.cacheReadTokens = tokens(
        at(raw, 'input_tokens_details', 'cached_tokens'),
      )
      usage.cacheWriteTokens = tokens(
        at(raw, 'input_tokens_details', 'cache_write_tokens'),
      )
      usage.reasoningTokens = tokens(
        at(raw, 'output_tokens_details', 'reasoning_tokens'),
      )
      const output = at(v, 'response', 'output')
      if (Array.isArray(output))
        for (const item of output)
          if (at(item, 'type') === 'reasoning') this.keep(item)
      this.done = true
      this.truncated = kind === 'response.incomplete'
    } else if (kind === 'response.failed')
      return error(at(v, 'response', 'error'))
    else if (kind === 'error') return error(v)
    return null
  }
}
