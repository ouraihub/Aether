import type {
  ImagePart,
  ModelRef,
  RunRequest,
} from '@ouraihub/aether-contracts'
import type { HistoryTurn } from '@ouraihub/aether-core/types'
import { badRequest } from '@ouraihub/aether-core/error'
import { canonicalJson, charCount } from '@ouraihub/aether-core/guard'
import {
  checkId,
  checkText,
  conflict,
  historyPath,
  idle,
  imageParts,
  notFound,
  target,
  validateImages,
  visibleText,
} from './model.ts'
import type { ConversationDocument, StoredImage, StoredTurn } from './model.ts'

export type Action =
  | { kind: 'reply'; parentId: string | null }
  | { kind: 'retry'; sourceId: string }
  | { kind: 'translation'; sourceId: string; targetLanguage: string }
export type SelectionGuard =
  | { kind: 'explicit' }
  | { kind: 'selected'; turnId: string }
  | { kind: 'latest'; turnId: string | null }
export interface ReservationInput {
  input: string
  images: ImagePart[]
  historyPath: StoredTurn[]
  history: HistoryTurn[]
  sourceModel: ModelRef | null
}
export interface PreparedAction extends ReservationInput {
  request: RunRequest
  effectiveRequest: RunRequest
  action: Action
  selectionGuard: SelectionGuard
  expected: string
  requiresSourceModel: boolean
}
function latest(document: ConversationDocument): string | null {
  return document.turns.findLast((t) => t.status === 'completed')?.id ?? null
}
export function authoritative(
  request: RunRequest,
  action: Action,
  guard: SelectionGuard,
  document: ConversationDocument | null,
): ReservationInput {
  if ((request.conversationId ?? null) !== (document?.summary.id ?? null))
    throw notFound('conversation')
  if (document) {
    idle(document)
    if (
      guard.kind === 'selected' &&
      document.summary.selectedTurnId !== guard.turnId
    )
      throw conflict('preparation_changed')
    if (
      guard.kind === 'latest' &&
      (document.summary.selectedTurnId !== null ||
        latest(document) !== guard.turnId)
    )
      throw conflict('preparation_changed')
  }
  let input = request.input,
    images = request.images ?? [],
    head: string | null,
    sourceModel: ModelRef | null = null
  if (action.kind === 'reply') {
    head = action.parentId
    if (
      head !== null &&
      (!document || target(document, head).status !== 'completed')
    )
      throw conflict('turn_not_completed')
  } else {
    if (!document) throw notFound('conversation')
    const source = target(document, action.sourceId)
    if (source.status === 'running') throw conflict('turn_running')
    sourceModel =
      source.endpointId !== null && source.modelId !== null
        ? { endpointId: source.endpointId, modelId: source.modelId }
        : null
    if (action.kind === 'retry') {
      input = source.input
      images = imageParts(source.images)
      head = source.parentId
    } else {
      const text = visibleText(source)
      if (!text.trim()) throw badRequest('translateTurnId', 'empty_reply')
      input = `Translate the following reply into ${action.targetLanguage}. Return only the translation, preserving Markdown formatting.\n\n${text}`
      images = []
      head = source.id
    }
  }
  checkText('input', input, 100_000)
  const path = document ? historyPath(document, head) : []
  return {
    input,
    images,
    historyPath: path,
    history: path.map((turn) => ({
      status: turn.status,
      input: turn.input,
      messages: turn.messages,
      images: imageParts(turn.images),
    })),
    sourceModel,
  }
}
async function digest(value: string): Promise<string> {
  const bytes = await crypto.subtle.digest(
    'SHA-256',
    new TextEncoder().encode(value),
  )
  return Array.from(new Uint8Array(bytes), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
}
async function imageBasis(
  turnId: string,
  kind: string,
  images: readonly StoredImage[],
): Promise<unknown[]> {
  return Promise.all(
    images.map(async (image, index) => ({
      turnId,
      kind,
      index,
      mime: image.mime,
      data: image.data === null ? null : await digest(image.data),
    })),
  )
}
export async function fingerprint(
  request: RunRequest,
  action: Action,
  guard: SelectionGuard,
  document: ConversationDocument | null,
  input: ReservationInput,
): Promise<string> {
  const source =
    action.kind !== 'reply' && document
      ? target(document, action.sourceId)
      : null
  const sourceBasis = source
    ? {
        id: source.id,
        status: source.status,
        parentId: source.parentId,
        questionId: source.questionId,
        supersedesId: source.supersedesId,
        kind: source.kind,
        modelId: source.modelId,
        endpointId: source.endpointId,
        input: source.input,
        visibleText: action.kind === 'translation' ? visibleText(source) : null,
        images: await imageBasis(source.id, 'input', source.images),
        generated: await imageBasis(source.id, 'generated', source.generated),
      }
    : null
  const path = await Promise.all(
    input.historyPath.map(async (turn) => ({
      id: turn.id,
      parentId: turn.parentId,
      status: turn.status,
      input: turn.input,
      messages: turn.messages,
      endpointId: turn.endpointId,
      modelId: turn.modelId,
      reasoningEffort: turn.reasoningEffort,
      images: await imageBasis(turn.id, 'input', turn.images),
      generated: await imageBasis(turn.id, 'generated', turn.generated),
    })),
  )
  const currentImages = await Promise.all(
    input.images.map(async (image, index) => ({
      index,
      mime: image.mime,
      data: await digest(image.data),
    })),
  )
  return digest(
    canonicalJson({
      version: 1,
      conversationId: request.conversationId ?? null,
      scope: document?.summary.scope ?? request.scope ?? null,
      action,
      guard,
      input: input.input,
      images: currentImages,
      source: sourceBasis,
      path,
    }),
  )
}
export async function resolveAction(
  request: RunRequest,
  document: ConversationDocument | null,
): Promise<PreparedAction> {
  for (const [field, id] of [
    ['conversationId', request.conversationId],
    ['replyToId', request.replyToId],
    ['retryTurnId', request.retryTurnId],
    ['translateTurnId', request.translateTurnId],
  ])
    if (id !== undefined && id !== null) {
      if (typeof id !== 'string') throw badRequest(String(field), 'invalid')
      checkId(String(field), id)
    }
  if (request.scope !== undefined && request.scope !== null)
    checkText('scope', request.scope, 200)
  checkText('input', request.input, 100_000)
  validateImages(request.images ?? [])
  const sourced = Boolean(
    request.retryTurnId || request.translateTurnId || request.regenerate,
  )
  if (
    Number(Boolean(request.retryTurnId)) +
      Number(Boolean(request.translateTurnId)) +
      Number(request.regenerate) >
    1
  )
    throw badRequest('action', 'multiple')
  if (sourced && request.replyToId)
    throw badRequest('replyToId', 'action_conflict')
  if (sourced && (request.images?.length ?? 0) > 0)
    throw badRequest('images', 'action_conflict')
  if (
    Boolean(request.translateTurnId) !==
    (request.targetLanguage !== null && request.targetLanguage !== undefined)
  )
    throw badRequest('targetLanguage', 'translation_required')
  if (!request.conversationId && (sourced || request.replyToId))
    throw badRequest('conversationId', 'required')
  if ((request.retryTurnId || request.translateTurnId) && request.webSearch)
    throw badRequest('webSearch', 'action_conflict')
  let guard: SelectionGuard = { kind: 'explicit' },
    head: string | null = request.replyToId ?? null
  if (
    document &&
    !request.replyToId &&
    !request.retryTurnId &&
    !request.translateTurnId
  ) {
    if (document.summary.selectedTurnId !== null) {
      head = document.summary.selectedTurnId
      guard = { kind: 'selected', turnId: head }
    } else {
      head = latest(document)
      guard = { kind: 'latest', turnId: head }
    }
  }
  let action: Action
  if (request.translateTurnId) {
    const language = request.targetLanguage?.trim() ?? ''
    if (!language || charCount(language) > 80)
      throw badRequest('targetLanguage', 'invalid')
    action = {
      kind: 'translation',
      sourceId: request.translateTurnId,
      targetLanguage: language,
    }
  } else if (request.retryTurnId)
    action = { kind: 'retry', sourceId: request.retryTurnId }
  else if (request.regenerate) {
    if (head === null) throw notFound('turn')
    action = { kind: 'retry', sourceId: head }
  } else action = { kind: 'reply', parentId: head }
  const input = authoritative(request, action, guard, document)
  const effectiveRequest: RunRequest = {
    ...request,
    input: input.input,
    images: input.images,
    ...(request.model ? {} : { model: input.sourceModel }),
  }
  return {
    ...input,
    request,
    effectiveRequest,
    action,
    selectionGuard: guard,
    expected: await fingerprint(request, action, guard, document, input),
    requiresSourceModel: action.kind !== 'reply' && !request.model,
  }
}
