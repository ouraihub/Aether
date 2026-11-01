import type { ImagePart, RunRequest } from '@ouraihub/aether-contracts'
import { badRequest, cancelled, KitFailure, safeError } from '@ouraihub/aether-core/error'
import { charCount } from '@ouraihub/aether-core/guard'
import { ImageFailure } from '@ouraihub/aether-core/images'
import type { McpSession } from '@ouraihub/aether-core/mcp'
import type { ResolvedSearchConfig } from '@ouraihub/aether-core/search'
import type { ToolCall, ToolExecution, ToolSpec, Connection } from '@ouraihub/aether-core/types'
import type { Actor, WorkerContext } from './context.js'
import type { AcceptedHandle, ConversationService, PreparedAction } from './conversations/index.js'
import type { WorkerHost } from './host.js'
import type { ImageService } from './images/index.js'
import type { McpService } from './mcp/index.js'
import type { ProviderService } from './providers/index.js'
import type { SearchService } from './search/index.js'

export interface Candidate {
  id: string
  image: ImagePart
  origin: string
}

export interface ToolPreparationOptions {
  host: WorkerHost
  providers: ProviderService
  search?: SearchService
  mcp?: McpService
  images?: ImageService
}

export interface PreparedTools {
  tools: ToolSpec[]
  mcp: McpSession | null
  painting: Connection | null
  candidates: Candidate[]
  search: ResolvedSearchConfig | null
  nativeWebSearch: boolean
}

export interface ToolRuntimeState {
  input: { tools: readonly ToolSpec[] }
  context: WorkerContext
  mcp: McpSession | null
  painting: Connection | null
  candidates: readonly Candidate[]
  paintingCalls: number
}

export const assertActive = (signal: AbortSignal): void => {
  if (signal.aborted) throw cancelled()
}

const missing = (field: string): KitFailure => badRequest(field, 'missing_instance')

function candidatesFor(action: PreparedAction, enabled: boolean): Candidate[] {
  if (!enabled) return []
  const result: Candidate[] = []
  const add = (images: readonly { mime: string; data: string | null }[], origin: string): void => {
    for (let i = images.length - 1; i >= 0 && result.length < 8; i--) {
      const image = images[i]
      if (image && image.data !== null)
        result.push({
          id: `image-${result.length + 1}`,
          image: { mime: image.mime, data: image.data },
          origin,
        })
    }
  }
  add(action.images, 'current attachment')
  for (let i = action.historyPath.length - 1; i >= 0 && result.length < 8; i--) {
    const turn = action.historyPath[i]
    if (turn) {
      add(turn.generated, 'historical generated image')
      add(turn.images, 'historical attachment')
    }
  }
  return result
}

function paintingTool(candidates: readonly Candidate[]): ToolSpec {
  const properties: Record<string, unknown> = {
    prompt: { type: 'string', minLength: 1, maxLength: 4000 },
    size: { type: 'string', enum: ['1024x1024', '1024x1536', '1536x1024'] },
  }
  if (candidates.length)
    properties.image_ids = {
      type: 'array',
      items: { type: 'string', enum: candidates.map((item) => item.id) },
      minItems: 1,
      maxItems: 4,
    }
  return {
    name: 'generate_image',
    description: `Generate or edit an image. At most two calls per run.${candidates.length ? ` Images you can pass in image_ids (newest first): ${candidates.map((item) => `${item.id}: ${item.origin}`).join('; ')}` : ''}`,
    parameters: { type: 'object', properties, required: ['prompt'], additionalProperties: false },
    approval: 'ask',
    effect: 'other',
    source: { kind: 'host' },
  }
}

export async function prepareTools(
  options: ToolPreparationOptions,
  effective: RunRequest,
  runId: string,
  preparationContext: WorkerContext,
  signal: AbortSignal,
  actor: Actor,
  action: PreparedAction,
  connection: Connection,
): Promise<PreparedTools> {
  const tools = structuredClone(await options.host.tools(effective, runId, preparationContext)).map(
    (tool) => {
      if (tool.source.kind !== 'host') throw badRequest('tools', 'host_source')
      return tool
    },
  )
  assertActive(signal)
  let mcp: McpSession | null = null
  if (options.mcp && (options.host.mcpTools?.(effective, preparationContext) ?? true)) {
    assertActive(signal)
    mcp = await options.mcp.client(preparationContext).session(signal)
  }
  assertActive(signal)
  if (mcp)
    tools.push(
      ...mcp.tools().map(
        (tool) =>
          ({
            name: tool.prefixed,
            description: tool.description,
            parameters: { ...tool.schema },
            approval: tool.approval,
            effect: 'other',
            source: { kind: 'mcp', server: tool.server },
          }) satisfies ToolSpec,
      ),
    )
  let painting: Connection | null = null
  if (effective.generateImage) {
    if (!options.images) throw missing('images')
    const reference = await options.host.paintingModel?.(effective, preparationContext)
    assertActive(signal)
    if (!reference)
      throw new KitFailure('needs_config', 'Painting model unavailable', {
        reason: 'painting_model',
      })
    painting = structuredClone(await options.providers.resolve(actor, reference, 'image'))
    assertActive(signal)
  }
  const candidates = candidatesFor(action, painting?.canVision ?? false)
  if (painting && connection.supportsTools !== false) tools.push(paintingTool(candidates))
  let search: ResolvedSearchConfig | null = null
  let nativeWebSearch = false
  if (effective.webSearch) {
    if (!options.search) throw missing('search')
    const preferences = await options.search.preferences(actor)
    assertActive(signal)
    nativeWebSearch =
      preferences.modelToolsPreferred &&
      connection.nativeWebSearch === true &&
      connection.protocol !== 'openai-chat'
    if (!nativeWebSearch) {
      search = {
        ...structuredClone(await options.search.resolve(actor, 'search')),
        preferences: structuredClone(preferences),
      }
      assertActive(signal)
    }
  }
  return { tools, mcp, painting, candidates, search, nativeWebSearch }
}

export async function callTool(
  handle: AcceptedHandle | null,
  state: ToolRuntimeState,
  call: ToolCall,
  signal: AbortSignal,
  options: ToolPreparationOptions & { conversations: ConversationService | null },
): Promise<ToolExecution> {
  assertActive(signal)
  const tool = state.input.tools.find((item) => item.name === call.name)
  if (!tool) return { kind: 'error', code: 'not_found', message: 'Tool not in this run' }
  if (tool.source.kind === 'mcp') {
    if (!state.mcp) return { kind: 'error', code: 'not_found', message: 'MCP tool not in this run' }
    const result = await state.mcp.call(call.name, call.args, signal)
    if (result.kind === 'unknown') return result
    if (result.kind === 'failed')
      return { kind: 'error', code: result.code, message: result.message }
    const text = result.content
      .map((item) =>
        item.type === 'text' && typeof item.text === 'string' ? item.text : JSON.stringify(item),
      )
      .join('\n')
    const payload = {
      content: result.content,
      ...(result.structuredContent ? { structuredContent: result.structuredContent } : {}),
    }
    return result.isError
      ? { kind: 'error', code: 'mcp_error', message: text, payload }
      : { kind: 'ok', text, payload }
  }
  if (call.name === 'generate_image' && state.painting)
    return paint(handle, state, call, signal, options)
  return options.host.callTool(call, signal, { ...state.context, signal })
}

async function paint(
  handle: AcceptedHandle | null,
  state: ToolRuntimeState,
  call: ToolCall,
  signal: AbortSignal,
  options: Pick<ToolPreparationOptions, 'images'> & { conversations: ConversationService | null },
): Promise<ToolExecution> {
  try {
    const { prompt, size = '1024x1024', image_ids: ids } = call.args
    if (
      Object.keys(call.args).some((key) => !['prompt', 'size', 'image_ids'].includes(key)) ||
      typeof prompt !== 'string' ||
      !prompt.trim() ||
      charCount(prompt) > 4000
    )
      throw badRequest('prompt', 'invalid')
    if (size !== '1024x1024' && size !== '1024x1536' && size !== '1536x1024')
      throw badRequest('size', 'invalid')
    const images: ImagePart[] = []
    if (ids !== undefined) {
      if (!state.painting?.canVision || !state.candidates.length)
        throw badRequest('image_ids', 'unsupported')
      if (!Array.isArray(ids) || ids.length === 0 || ids.length > 4)
        throw badRequest('image_ids', 'too_many')
      for (const id of ids) {
        const candidate = state.candidates.find((item) => item.id === id)
        if (!candidate) throw badRequest('image_ids', 'unknown')
        images.push(candidate.image)
      }
    }
    if (state.paintingCalls >= 2) throw badRequest('generateImage', 'call_limit')
    assertActive(signal)
    if (!state.painting || !options.images) throw missing('images')
    state.paintingCalls++
    const result = await options.images.generate(
      state.painting,
      { prompt, size, images },
      { ...state.context, signal },
    )
    let references
    try {
      references =
        handle && options.conversations
          ? await options.conversations.addGenerated(handle, [result.image], state.context.actor)
          : [result.reference]
    } catch {
      return {
        kind: 'error',
        code: 'generated_save_failed',
        message:
          'Image was generated, but its persistence could not be confirmed. Do not automatically regenerate.',
        payload: {
          generated: true,
          persisted: false,
          usage: result.usage,
          totalTokens: result.totalTokens,
          revisedPrompt: result.revisedPrompt,
        },
      }
    }
    return {
      kind: 'ok',
      text: 'Generated 1 image(s).',
      payload: {
        images: references,
        usage: result.usage,
        totalTokens: result.totalTokens,
        revisedPrompt: result.revisedPrompt,
      },
    }
  } catch (error) {
    if (error instanceof ImageFailure && error.dispatch === 'unknown')
      return {
        kind: 'unknown',
        reason:
          'Image generation was dispatched; its result could not be confirmed. Do not automatically retry.',
      }
    if (error instanceof ImageFailure && error.dispatch === 'generated')
      return {
        kind: 'error',
        code: 'generated_save_failed',
        message: 'Image was generated, but its persistence could not be confirmed.',
        payload: { generated: true, persisted: false },
      }
    const failure = safeError(error)
    return {
      kind: 'error',
      code: failure.code,
      message: error instanceof KitFailure ? error.message : 'Image operation failed',
    }
  }
}
