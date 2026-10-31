import type {
  Conversation,
  ConversationSummary,
  RunAccepted,
  RunRequest,
} from '@ouraihub/aether-contracts'
import type { RunInput } from '@ouraihub/aether-core/types'
import { KitFailure } from '@ouraihub/aether-core/error'
import { canonicalJson } from '@ouraihub/aether-core/guard'
import type { Actor } from '../context.ts'
import type { DocumentStore, StoredDocument } from '../storage.ts'
import { authoritative, fingerprint, resolveAction } from './action.ts'
import type { PreparedAction, ReservationInput } from './action.ts'
import {
  COLLECTION,
  checkId,
  checkText,
  conflict,
  emptyUsage,
  notFound,
  parseDocument,
  project,
  target,
} from './model.ts'
import type { ConversationDocument, StoredTurn } from './model.ts'

/** Persist this before reservation; it contains identity only, never provider credentials. */
export interface AcceptedHandle {
  runId: string
  conversationId: string
  turnId: string
  reservationId: string
}
export interface TurnReservation {
  handle: AcceptedHandle
  accepted: RunAccepted
  input: ReservationInput
  runInput: RunInput
}
export interface ConversationOptions {
  store: DocumentStore
  now?: () => number
  newId?: () => string
}
export class ConversationService {
  protected readonly store: DocumentStore
  protected readonly now: () => number
  protected readonly newId: () => string
  constructor(options: ConversationOptions) {
    this.store = options.store
    this.now = options.now ?? Date.now
    this.newId = options.newId ?? (() => crypto.randomUUID())
  }
  protected async load(
    id: string,
    actor: Actor,
  ): Promise<{ revision: number; document: ConversationDocument }> {
    checkId('conversationId', id)
    const stored = await this.store.get(actor, COLLECTION, id)
    if (!stored) throw notFound('conversation')
    return {
      revision: stored.revision,
      document: parseDocument(stored.value, id),
    }
  }
  protected async *documents(actor: Actor): AsyncGenerator<StoredDocument> {
    let after: string | undefined
    while (true) {
      const page = await this.store.list(actor, COLLECTION, {
        ...(after === undefined ? {} : { after }),
        limit: 200,
      })
      for (const stored of page) yield stored
      if (page.length < 200) return
      const last = page.at(-1)?.key
      if (last === undefined || (after !== undefined && last <= after))
        throw new KitFailure('internal', 'Invalid document pagination')
      after = last
    }
  }
  async list(
    scope: string | null,
    actor: Actor,
  ): Promise<ConversationSummary[]> {
    if (scope !== null) checkText('scope', scope, 200)
    const summaries: ConversationSummary[] = []
    for await (const stored of this.documents(actor)) {
      const summary = parseDocument(stored.value, stored.key).summary
      if (scope === null || summary.scope === scope) summaries.push(summary)
    }
    return summaries
      .sort((a, b) => b.updatedAt - a.updatedAt || b.id.localeCompare(a.id))
      .slice(0, 200)
  }
  async get(id: string, actor: Actor): Promise<Conversation> {
    return project((await this.load(id, actor)).document)
  }
  async resolve(request: RunRequest, actor: Actor): Promise<PreparedAction> {
    const snapshot = structuredClone(request)
    const document = snapshot.conversationId
      ? (await this.load(snapshot.conversationId, actor)).document
      : null
    return resolveAction(snapshot, document)
  }
  plan(prepared: PreparedAction, runId: string): AcceptedHandle {
    checkId('runId', runId)
    const conversationId =
      prepared.request.conversationId ?? `${runId}-conversation`
    checkId('conversationId', conversationId)
    return { runId, conversationId, turnId: runId, reservationId: runId }
  }
  async reserve(
    prepared: PreparedAction,
    validatedInput: RunInput,
    runId: string,
    actor: Actor,
    validate?: (input: ReservationInput, execution: RunInput) => RunInput,
  ): Promise<TurnReservation> {
    const handle = this.plan(prepared, runId)
    for (let attempt = 0; attempt < 8; attempt++) {
      const stored = await this.store.get(
        actor,
        COLLECTION,
        handle.conversationId,
      )
      if (prepared.request.conversationId && !stored)
        throw conflict('preparation_changed')
      if (!prepared.request.conversationId && stored)
        throw conflict('run_identity_exists')
      const document = stored ? parseDocument(stored.value, stored.key) : null
      let input: ReservationInput
      try {
        input = authoritative(
          prepared.request,
          prepared.action,
          prepared.selectionGuard,
          document,
        )
      } catch (error) {
        if (
          error instanceof KitFailure &&
          error.details?.reason === 'turn_running'
        )
          throw error
        throw conflict('preparation_changed')
      }
      if (
        (await fingerprint(
          prepared.request,
          prepared.action,
          prepared.selectionGuard,
          document,
          input,
        )) !== prepared.expected
      )
        throw conflict('preparation_changed')
      const runInput = validate
        ? validate(input, validatedInput)
        : validatedInput
      if (
        runInput.runId !== runId ||
        runInput.input !== input.input ||
        canonicalJson(runInput.images) !== canonicalJson(input.images)
      )
        throw conflict('validated_input_changed')
      if (
        prepared.requiresSourceModel &&
        (!input.sourceModel ||
          runInput.connection.endpointId !== input.sourceModel.endpointId ||
          runInput.connection.modelId !== input.sourceModel.modelId)
      )
        throw new KitFailure('needs_config', 'Source model unavailable', {
          reason: 'source_model_unavailable',
        })
      const now = this.now(),
        source =
          document && prepared.action.kind !== 'reply'
            ? target(document, prepared.action.sourceId)
            : null
      const parentId =
        prepared.action.kind === 'reply'
          ? prepared.action.parentId
          : prepared.action.kind === 'translation'
            ? (source?.id ?? null)
            : (source?.parentId ?? null)
      const turn: StoredTurn = {
        id: handle.turnId,
        seq: (document?.turns.at(-1)?.seq ?? 0) + 1,
        runId,
        reservationId: handle.reservationId,
        parentId,
        questionId:
          prepared.action.kind === 'retry' && source
            ? source.questionId
            : handle.turnId,
        supersedesId:
          prepared.action.kind === 'retry' ? (source?.id ?? null) : null,
        kind:
          prepared.action.kind === 'translation'
            ? 'translation'
            : (source?.kind ?? 'reply'),
        status: 'running',
        input: input.input,
        messages: [],
        modelId: runInput.connection.modelId,
        endpointId: runInput.connection.endpointId,
        skills: structuredClone(prepared.request.skills),
        errorCode: null,
        usage: emptyUsage(),
        contextUsage: null,
        reasoningEffort: runInput.reasoningEffort,
        cost: null,
        createdAt: now,
        images:
          prepared.action.kind === 'retry' && source
            ? structuredClone(source.images)
            : input.images.map((image) => ({ ...image, createdAt: now })),
        generated: [],
      }
      const next: ConversationDocument = document ?? {
        version: 1,
        summary: {
          id: handle.conversationId,
          title: [...input.input.replace(/[\r\n]/gu, '')].slice(0, 40).join(''),
          scope: prepared.request.scope ?? null,
          selectedTurnId: null,
          createdAt: now,
          updatedAt: now,
        },
        turns: [],
      }
      next.turns.push(turn)
      next.summary.updatedAt = now
      const checked = parseDocument(next, handle.conversationId)
      try {
        await this.store.compareAndSet(
          actor,
          COLLECTION,
          handle.conversationId,
          stored?.revision ?? null,
          checked,
        )
      } catch (error) {
        if (
          error instanceof KitFailure &&
          error.code === 'conflict' &&
          attempt < 7
        )
          continue
        throw error
      }
      return {
        handle,
        accepted: {
          runId,
          conversationId: handle.conversationId,
          turnId: handle.turnId,
        },
        input,
        runInput,
      }
    }
    throw conflict('conversation_contended')
  }
}
