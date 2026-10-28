import type { ImagePart, Protocol, ReasoningConfig, ReasoningEffort } from '@ouraihub/aether-contracts'
import type { Message, ToolSpec } from '../types.js'
import { at, isRecord } from '../guard.js'

export const PROTOCOL_PATH: Record<Protocol, string> = {
  'anthropic-messages': 'messages',
  'openai-chat': 'chat/completions',
  'openai-responses': 'responses',
}

export function authHeaders(protocol: Protocol, key: string | null): Record<string, string> {
  const headers: Record<string, string> = {}
  if (key) headers.authorization = `Bearer ${key}`
  if (protocol === 'anthropic-messages') {
    if (key) headers['x-api-key'] = key
    headers['anthropic-version'] = '2023-06-01'
  }
  return headers
}

export interface ModelRequest {
  protocol: Protocol
  model: string
  system: string
  history: readonly Message[]
  input: string
  images?: readonly ImagePart[]
  maxOutput: number
  endpointId: string | null
  /** Canonical endpoint API base; decides vendor-only fields such as the Responses `summary`. */
  baseUrl: string
  reasoningEffort: ReasoningEffort
  reasoning: ReasoningConfig | null
  reasoningBudget: number | null
  nativeWebSearch?: boolean
  /** Host tools offered this round. */
  tools?: readonly Pick<ToolSpec, 'name' | 'description' | 'parameters'>[]
  /** Messages this run already produced after `input` (assistant tool calls and their results). */
  turn?: readonly Message[]
}

function userContent(protocol: Protocol, text: string, images: readonly ImagePart[] | undefined): unknown {
  if (!images?.length) return text
  const url = (image: ImagePart): string => `data:${image.mime};base64,${image.data}`
  if (protocol === 'anthropic-messages')
    return [
      ...images.map((image) => ({
        type: 'image',
        source: { type: 'base64', media_type: image.mime, data: image.data },
      })),
      { type: 'text', text },
    ]
  if (protocol === 'openai-chat')
    return [{ type: 'text', text }, ...images.map((image) => ({ type: 'image_url', image_url: { url: url(image) } }))]
  return [{ type: 'input_text', text }, ...images.map((image) => ({ type: 'input_image', image_url: url(image) }))]
}

export function requestBody(request: ModelRequest): Record<string, unknown> {
  const messages: Record<string, unknown>[] = []
  const anthropic = request.protocol === 'anthropic-messages',
    responses = request.protocol === 'openai-responses'
  const encode = (message: Message): void => {
    if ('user' in message) {
      messages.push({ role: 'user', content: userContent(request.protocol, message.user, message.images) })
      return
    }
    if ('tool' in message) {
      const { call_id, text } = message.tool
      if (responses) {
        messages.push({ type: 'function_call_output', call_id, output: text })
        return
      }
      if (!anthropic) {
        messages.push({ role: 'tool', tool_call_id: call_id, content: text })
        return
      }
      // Consecutive results answer one assistant message: one user message of `tool_result` blocks.
      const block = { type: 'tool_result', tool_use_id: call_id, content: text }
      const last = messages.at(-1)
      if (
        last?.role === 'user' &&
        Array.isArray(last.content) &&
        last.content.every((item) => at(item, 'type') === 'tool_result')
      )
        last.content.push(block)
      else messages.push({ role: 'user', content: [block] })
      return
    }
    const assistant = message.assistant
    const calls = assistant.tool_calls ?? []
    const reasoning = (assistant.reasoning ?? []).filter(
      (block) =>
        block.protocol === request.protocol &&
        block.endpointId === request.endpointId &&
        block.modelId === request.model,
    )
    if (anthropic) {
      const blocks = reasoning.flatMap((block) => (block.value && typeof block.value === 'object' ? [block.value] : []))
      if (assistant.text) blocks.push({ type: 'text', text: assistant.text })
      for (const call of calls) blocks.push({ type: 'tool_use', id: call.id, name: call.name, input: call.args ?? {} })
      if (blocks.length) messages.push({ role: 'assistant', content: blocks })
    } else if (responses) {
      for (const block of reasoning) if (isRecord(block.value)) messages.push(block.value)
      if (assistant.text) messages.push({ role: 'assistant', content: assistant.text })
      for (const call of calls)
        messages.push({
          type: 'function_call',
          call_id: call.id,
          name: call.name,
          arguments: JSON.stringify(call.args ?? {}),
        })
    } else if (assistant.text || calls.length) {
      messages.push({
        role: 'assistant',
        content: assistant.text,
        ...(reasoning.length
          ? {
              reasoning_content: reasoning
                .map((block) => (typeof block.value === 'string' ? block.value : ''))
                .join(''),
            }
          : {}),
        ...(calls.length
          ? {
              tool_calls: calls.map((call) => ({
                id: call.id,
                type: 'function',
                function: { name: call.name, arguments: JSON.stringify(call.args ?? {}) },
              })),
            }
          : {}),
      })
    }
  }
  for (const message of request.history) encode(message)
  encode({ user: request.input, ...(request.images ? { images: [...request.images] } : {}) })
  for (const message of request.turn ?? []) encode(message)
  const { model, system, maxOutput, reasoningEffort: effort } = request
  let body: Record<string, unknown>
  if (request.protocol === 'anthropic-messages') {
    body = { model, max_tokens: maxOutput, stream: true, messages, ...(system ? { system } : {}) }
    if (effort === 'none') body.thinking = { type: 'disabled' }
    else if (effort !== 'default' && request.reasoning?.dialect === 'anthropic-budget')
      body.thinking = { type: 'enabled', budget_tokens: request.reasoningBudget }
    else if (effort !== 'default') {
      body.thinking = { type: 'adaptive' }
      body.output_config = { effort }
    }
  } else if (request.protocol === 'openai-chat') {
    if (system) messages.unshift({ role: 'system', content: system })
    body = { model, max_tokens: maxOutput, stream: true, stream_options: { include_usage: true }, messages }
    const dialect = request.reasoning?.dialect
    if (effort !== 'default' && dialect === 'thinking-type') {
      body.thinking = { type: effort === 'none' ? 'disabled' : 'enabled' }
      if (effort !== 'none' && effort !== 'auto') body.reasoning_effort = effort
    } else if (effort !== 'default' && dialect === 'qwen') {
      body.enable_thinking = effort !== 'none'
      if (request.reasoningBudget !== null) body.thinking_budget = request.reasoningBudget
    } else if (effort !== 'default') body.reasoning_effort = effort
  } else {
    // Third-party Responses servers may reject `summary` with 400.
    const summary = /^https:\/\/api\.openai\.com(?:\/|$)/.test(request.baseUrl) ? { summary: 'auto' } : {}
    body = {
      model,
      max_output_tokens: maxOutput,
      stream: true,
      input: messages,
      store: false,
      include: ['reasoning.encrypted_content'],
      ...(system ? { instructions: system } : {}),
      ...(effort === 'default' ? {} : { reasoning: { effort, ...summary } }),
    }
  }
  const tools: Record<string, unknown>[] = (request.tools ?? []).map(({ name, description, parameters }) =>
    anthropic
      ? { name, description, input_schema: parameters }
      : responses
        ? { type: 'function', name, description, parameters }
        : { type: 'function', function: { name, description, parameters } },
  )
  if (request.nativeWebSearch) {
    if (request.protocol === 'openai-chat') throw new Error('Native search is unsupported on Chat Completions')
    tools.push(anthropic ? { type: 'web_search_20250305', name: 'web_search', max_uses: 5 } : { type: 'web_search' })
  }
  if (tools.length) body.tools = tools
  return body
}
