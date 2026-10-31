import type {
  ContextUsage,
  Conversation,
  ConversationSummary,
  Cost,
  ImagePart,
  ImageRef,
  ReasoningEffort,
  SkillRef,
  TokenUsage,
  TurnStatus,
} from '@ouraihub/aether-contracts'
import type {
  Message,
  ReasoningBlock,
  ToolCall,
} from '@ouraihub/aether-core/types'
import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import { charCount, isRecord } from '@ouraihub/aether-core/guard'
import {
  decodeImage,
  IMAGE_MIMES,
  parseImage,
} from '@ouraihub/aether-core/images'

export const COLLECTION = 'conversations'
export const IMAGE_TTL_DAYS = 7
export interface StoredImage {
  mime: string
  data: string | null
  createdAt: number
}
export interface StoredTurn {
  id: string
  seq: number
  runId: string
  reservationId: string
  parentId: string | null
  questionId: string
  supersedesId: string | null
  kind: 'reply' | 'translation'
  status: TurnStatus
  input: string
  messages: Message[]
  modelId: string | null
  endpointId: string | null
  skills: SkillRef[]
  errorCode: string | null
  usage: TokenUsage
  contextUsage: ContextUsage | null
  reasoningEffort: ReasoningEffort | null
  cost: Cost | null
  createdAt: number
  images: StoredImage[]
  generated: StoredImage[]
}
export interface ConversationDocument {
  version: 1
  summary: ConversationSummary
  turns: StoredTurn[]
}
export const conflict = (reason: string): KitFailure =>
  new KitFailure('conflict', 'Conversation changed or unavailable', { reason })
export const notFound = (entity: string): KitFailure =>
  new KitFailure('not_found', 'Resource not found', { entity })
const corrupt = (): never => {
  throw new KitFailure('incompatible_db', 'Invalid stored conversation')
}
export const emptyUsage = (): TokenUsage => ({
  inputTokens: null,
  outputTokens: null,
  cacheReadTokens: null,
  cacheWriteTokens: null,
  reasoningTokens: null,
})
export function checkId(field: string, id: string): void {
  if (!id.trim() || charCount(id) > 128) throw badRequest(field, 'invalid')
}
export function checkText(
  field: string,
  text: string,
  limit: number,
  nonempty = false,
): void {
  if (charCount(text) > limit) throw badRequest(field, 'too_long', limit)
  if (nonempty && !text.trim()) throw badRequest(field, 'empty')
}
function record(value: unknown): Record<string, unknown> {
  return isRecord(value) ? value : corrupt()
}
function text(value: unknown): string {
  return typeof value === 'string' ? value : corrupt()
}
function nullableText(value: unknown): string | null {
  return value === null ? null : text(value)
}
function integer(value: unknown): number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
    ? value
    : corrupt()
}
function count(value: unknown): number | null {
  const parsed = value === null ? null : integer(value)
  return parsed === null || parsed <= 0xffff_ffff ? parsed : corrupt()
}
function array<T>(value: unknown, parse: (item: unknown) => T): T[] {
  return Array.isArray(value) ? value.map(parse) : corrupt()
}
function effort(value: unknown): ReasoningEffort | null {
  switch (value) {
    case null:
    case 'default':
    case 'none':
    case 'auto':
    case 'minimal':
    case 'low':
    case 'medium':
    case 'high':
    case 'xhigh':
    case 'max':
      return value
    default:
      return corrupt()
  }
}
function usage(value: unknown): TokenUsage {
  const v = record(value)
  return {
    inputTokens: count(v.inputTokens),
    outputTokens: count(v.outputTokens),
    cacheReadTokens: count(v.cacheReadTokens),
    cacheWriteTokens: count(v.cacheWriteTokens),
    reasoningTokens: count(v.reasoningTokens),
  }
}
function context(value: unknown): ContextUsage | null {
  if (value === null) return null
  const v = record(value),
    source = v.source
  if (source !== 'reported' && source !== 'estimated' && source !== 'unknown')
    return corrupt()
  return {
    tokens: count(v.tokens),
    contextWindow: count(v.contextWindow),
    source,
    endpointId: nullableText(v.endpointId),
    modelId: nullableText(v.modelId),
  }
}
function cost(value: unknown): Cost | null {
  if (value === null) return null
  const v = record(value),
    currency = v.currency
  if (
    typeof v.amount !== 'number' ||
    !Number.isFinite(v.amount) ||
    v.amount < 0 ||
    (currency !== 'USD' && currency !== 'CNY')
  )
    return corrupt()
  return { amount: v.amount, currency }
}
function image(value: unknown): StoredImage {
  const v = record(value),
    mime = text(v.mime),
    data = nullableText(v.data)
  if (!IMAGE_MIMES.some((value) => value === mime)) return corrupt()
  if (data !== null) {
    try {
      decodeImage({ mime, data })
    } catch {
      return corrupt()
    }
  }
  return { mime, data, createdAt: integer(v.createdAt) }
}
export function validateImages(images: readonly ImagePart[]): void {
  if (images.length > 4) throw badRequest('images', 'too_many', 4)
  for (const image of images) parseImage(image)
}
function reasoning(value: unknown): ReasoningBlock {
  const v = record(value),
    protocol = v.protocol
  if (
    protocol !== 'openai-chat' &&
    protocol !== 'openai-responses' &&
    protocol !== 'anthropic-messages'
  )
    return corrupt()
  return {
    protocol,
    endpointId: nullableText(v.endpointId),
    modelId: nullableText(v.modelId),
    value: json(v.value),
  }
}
function json(value: unknown, depth = 0): unknown {
  if (depth > 64) return corrupt()
  if (
    value === null ||
    typeof value === 'string' ||
    typeof value === 'boolean' ||
    (typeof value === 'number' && Number.isFinite(value))
  )
    return value
  if (Array.isArray(value))
    return value.map((item: unknown) => json(item, depth + 1))
  if (isRecord(value))
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, json(item, depth + 1)]),
    )
  return corrupt()
}
function call(value: unknown): ToolCall {
  const v = record(value)
  return { id: text(v.id), name: text(v.name), args: jsonRecord(v.args) }
}
function jsonRecord(value: unknown): Record<string, unknown> {
  const v = record(value)
  return Object.fromEntries(
    Object.entries(v).map(([key, item]) => [key, json(item)]),
  )
}
export function parseMessage(value: unknown): Message {
  const v = record(value)
  if (
    ['user', 'assistant', 'tool'].filter((key) => Object.hasOwn(v, key))
      .length !== 1
  )
    return corrupt()
  if (typeof v.user === 'string') {
    if (v.images === undefined) return { user: v.user }
    return {
      user: v.user,
      images: array(v.images, (item) => {
        try {
          return parseImage(item)
        } catch {
          return corrupt()
        }
      }),
    }
  }
  if (isRecord(v.assistant)) {
    const a = v.assistant
    return {
      assistant: {
        text: text(a.text),
        tool_calls: array(a.tool_calls, call),
        ...(a.reasoning === undefined
          ? {}
          : { reasoning: array(a.reasoning, reasoning) }),
      },
    }
  }
  if (isRecord(v.tool))
    return { tool: { call_id: text(v.tool.call_id), text: text(v.tool.text) } }
  return corrupt()
}
export function parseDocument(
  value: unknown,
  key: string,
): ConversationDocument {
  const v = record(value),
    s = record(v.summary)
  if (v.version !== 1 || s.id !== key) return corrupt()
  const summary: ConversationSummary = {
    id: text(s.id),
    title: text(s.title),
    scope: nullableText(s.scope),
    selectedTurnId: nullableText(s.selectedTurnId),
    createdAt: integer(s.createdAt),
    updatedAt: integer(s.updatedAt),
  }
  if (
    !summary.id.trim() ||
    charCount(summary.id) > 128 ||
    charCount(summary.title) > 200 ||
    (summary.scope !== null && charCount(summary.scope) > 200)
  )
    return corrupt()
  const turns = array(v.turns, (item): StoredTurn => {
    const t = record(item),
      status = t.status,
      kind = t.kind
    if (
      (status !== 'running' &&
        status !== 'completed' &&
        status !== 'failed' &&
        status !== 'cancelled') ||
      (kind !== 'reply' && kind !== 'translation')
    )
      return corrupt()
    return {
      id: text(t.id),
      seq: integer(t.seq),
      runId: text(t.runId),
      reservationId: text(t.reservationId),
      parentId: nullableText(t.parentId),
      questionId: text(t.questionId),
      supersedesId: nullableText(t.supersedesId),
      kind,
      status,
      input: text(t.input),
      messages: array(t.messages, parseMessage),
      modelId: nullableText(t.modelId),
      endpointId: nullableText(t.endpointId),
      skills: array(t.skills, (item) => {
        const skill = record(item)
        return { id: text(skill.id), revision: integer(skill.revision) }
      }),
      errorCode: nullableText(t.errorCode),
      usage: usage(t.usage),
      contextUsage: context(t.contextUsage),
      reasoningEffort: effort(t.reasoningEffort),
      cost: cost(t.cost),
      createdAt: integer(t.createdAt),
      images: array(t.images, image),
      generated: array(t.generated, image),
    }
  })
  const ids = new Set<string>(),
    seqs = new Set<number>()
  let running = 0,
    last = 0
  for (const t of turns) {
    if (
      !t.id.trim() ||
      charCount(t.id) > 128 ||
      ids.has(t.id) ||
      seqs.has(t.seq) ||
      t.seq <= last
    )
      return corrupt()
    if (
      !t.questionId.trim() ||
      charCount(t.questionId) > 128 ||
      !t.runId.trim() ||
      !t.reservationId.trim() ||
      charCount(t.input) > 100_000 ||
      t.images.length > 4 ||
      t.skills.length > 20
    )
      return corrupt()
    ids.add(t.id)
    seqs.add(t.seq)
    last = t.seq
    if (t.status === 'running') running++
  }
  if (
    running > 1 ||
    (summary.selectedTurnId !== null && !ids.has(summary.selectedTurnId))
  )
    return corrupt()
  for (const t of turns) {
    if (
      (t.parentId !== null && !ids.has(t.parentId)) ||
      (t.supersedesId !== null &&
        (!ids.has(t.supersedesId) || t.supersedesId === t.id))
    )
      return corrupt()
    const seen = new Set<string>()
    let node: StoredTurn | undefined = t
    while (node) {
      if (seen.has(node.id)) return corrupt()
      seen.add(node.id)
      node = turns.find((p) => p.id === node?.parentId)
    }
  }
  return { version: 1, summary, turns }
}
export function idle(document: ConversationDocument): void {
  if (document.turns.some((t) => t.status === 'running'))
    throw conflict('turn_running')
}
export function target(document: ConversationDocument, id: string): StoredTurn {
  checkId('turnId', id)
  const turn = document.turns.find((t) => t.id === id)
  if (!turn) throw notFound('turn')
  return turn
}
export function visibleText(turn: StoredTurn): string {
  return turn.messages
    .flatMap((m) =>
      'assistant' in m && m.assistant.text ? [m.assistant.text] : [],
    )
    .join('\n\n')
}
export function reasoningText(turn: StoredTurn): string {
  let text = ''
  for (const message of turn.messages) {
    if (!('assistant' in message)) continue
    for (const block of message.assistant.reasoning ?? []) {
      if (block.protocol === 'openai-chat' && typeof block.value === 'string')
        text += block.value
      else if (
        block.protocol === 'anthropic-messages' &&
        isRecord(block.value) &&
        block.value.type === 'thinking' &&
        typeof block.value.thinking === 'string'
      )
        text += block.value.thinking
      else if (
        block.protocol === 'openai-responses' &&
        isRecord(block.value) &&
        Array.isArray(block.value.summary)
      ) {
        for (const part of block.value.summary)
          if (
            isRecord(part) &&
            part.type === 'summary_text' &&
            typeof part.text === 'string'
          )
            text += part.text
      }
    }
  }
  return text
}
export function historyPath(
  document: ConversationDocument,
  head: string | null,
): StoredTurn[] {
  const path: StoredTurn[] = [],
    seen = new Set<string>()
  while (head !== null) {
    if (seen.has(head)) throw conflict('history_cycle')
    seen.add(head)
    const turn = target(document, head)
    path.push(turn)
    head = turn.parentId
  }
  return path.reverse()
}
export function imageParts(images: readonly StoredImage[]): ImagePart[] {
  return images.flatMap((i) =>
    i.data === null ? [] : [{ mime: i.mime, data: i.data }],
  )
}
export function imageRefs(
  conversationId: string,
  turnId: string,
  kind: 'input' | 'generated',
  images: readonly StoredImage[],
): ImageRef[] {
  return images.map((image, index) => ({
    mime: image.mime,
    url:
      image.data === null
        ? null
        : `/api/v1/conversations/${encodeURIComponent(conversationId)}/turns/${encodeURIComponent(turnId)}/images/${kind}/${index}`,
  }))
}
export function project(document: ConversationDocument): Conversation {
  return {
    summary: document.summary,
    turns: document.turns.map((t) => ({
      id: t.id,
      parentId: t.parentId,
      questionId: t.questionId,
      supersedesId: t.supersedesId,
      kind: t.kind,
      status: t.status,
      input: t.input,
      text: visibleText(t),
      modelId: t.modelId,
      endpointId: t.endpointId,
      errorCode: t.errorCode,
      usage: t.usage,
      contextUsage: t.contextUsage,
      reasoningEffort: t.reasoningEffort,
      cost: t.cost,
      reasoningText: reasoningText(t),
      images: imageRefs(document.summary.id, t.id, 'input', t.images),
      generated: imageRefs(document.summary.id, t.id, 'generated', t.generated),
    })),
  }
}
