import type {
  Conversation,
  ConversationSummary,
  ImagePart,
  ImageRef,
  RunAccepted,
  RunRequest,
} from '@ouraihub/aether-contracts'
import type { RunInput, RunSummary } from '@ouraihub/aether-core/types'
import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import { canonicalJson, isRecord } from '@ouraihub/aether-core/guard'
import { decodeImage, parseImage } from '@ouraihub/aether-core/images'
import type { Actor, WorkerContext } from '../context.ts'
import { retryCas } from '../storage.ts'
import type { DocumentStore, RetryOptions, StoredDocument } from '../storage.ts'
import { authoritative, fingerprint, resolveAction } from './action.ts'
import type { PreparedAction, ReservationInput } from './action.ts'
import {
  COLLECTION,
  checkId,
  checkText,
  conflict,
  emptyUsage,
  historyPath,
  idle,
  imageRefs,
  notFound,
  parseDocument,
  project,
  target,
  visibleText,
} from './model.ts'
import type { ConversationDocument, StoredTurn } from './model.ts'
export type { PreparedAction, ReservationInput } from './action.ts'
export type { ConversationDocument, StoredImage, StoredTurn } from './model.ts'
export { IMAGE_TTL_DAYS } from './model.ts'

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
  /** CAS retry schedule; tests inject sleep/random to make contention deterministic. */
  retry?: RetryOptions
}
export class ConversationService {
  private readonly store: DocumentStore
  private readonly now: () => number
  private readonly newId: () => string
  private readonly retry: RetryOptions
  constructor(options: ConversationOptions) {
    this.store = options.store
    this.now = options.now ?? Date.now
    this.newId = options.newId ?? (() => crypto.randomUUID())
    this.retry = options.retry ?? {}
  }
  /** Reloads and re-applies `attempt` on revision conflicts; see `retryCas`. */
  private update<T>(attempt: Parameters<typeof retryCas<T>>[0]): Promise<T> {
    return retryCas(attempt, () => conflict('conversation_contended'), this.retry)
  }
  private async load(id: string, actor: Actor): Promise<{ revision: number; document: ConversationDocument }> {
    checkId('conversationId', id)
    const stored = await this.store.get(actor, COLLECTION, id)
    if (!stored) throw notFound('conversation')
    return { revision: stored.revision, document: parseDocument(stored.value, id) }
  }
  private async *documents(actor: Actor): AsyncGenerator<StoredDocument> {
    let after: string | undefined
    while (true) {
      const page = await this.store.list(actor, COLLECTION, { ...(after === undefined ? {} : { after }), limit: 200 })
      for (const stored of page) yield stored
      if (page.length < 200) return
      const last = page.at(-1)?.key
      if (last === undefined || (after !== undefined && last <= after))
        throw new KitFailure('internal', 'Invalid document pagination')
      after = last
    }
  }
  async list(scope: string | null, actor: Actor): Promise<ConversationSummary[]> {
    if (scope !== null) checkText('scope', scope, 200)
    const summaries: ConversationSummary[] = []
    for await (const stored of this.documents(actor)) {
      const summary = parseDocument(stored.value, stored.key).summary
      if (scope === null || summary.scope === scope) summaries.push(summary)
    }
    return summaries.sort((a, b) => b.updatedAt - a.updatedAt || b.id.localeCompare(a.id)).slice(0, 200)
  }
  async get(id: string, actor: Actor): Promise<Conversation> {
    return project((await this.load(id, actor)).document)
  }
  async resolve(request: RunRequest, actor: Actor): Promise<PreparedAction> {
    const snapshot = structuredClone(request)
    const document = snapshot.conversationId ? (await this.load(snapshot.conversationId, actor)).document : null
    return resolveAction(snapshot, document)
  }
  plan(prepared: PreparedAction, runId: string): AcceptedHandle {
    checkId('runId', runId)
    const conversationId = prepared.request.conversationId ?? `${runId}-conversation`
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
    return this.update(async (cas) => {
      const stored = await this.store.get(actor, COLLECTION, handle.conversationId)
      if (prepared.request.conversationId && !stored) throw conflict('preparation_changed')
      if (!prepared.request.conversationId && stored) throw conflict('run_identity_exists')
      const document = stored ? parseDocument(stored.value, stored.key) : null
      let input: ReservationInput
      try {
        input = authoritative(prepared.request, prepared.action, prepared.selectionGuard, document)
      } catch (error) {
        if (error instanceof KitFailure && error.details?.reason === 'turn_running') throw error
        throw conflict('preparation_changed')
      }
      if (
        (await fingerprint(prepared.request, prepared.action, prepared.selectionGuard, document, input)) !==
        prepared.expected
      )
        throw conflict('preparation_changed')
      const runInput = validate ? validate(input, validatedInput) : validatedInput
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
        throw new KitFailure('needs_config', 'Source model unavailable', { reason: 'source_model_unavailable' })
      const now = this.now(),
        source = document && prepared.action.kind !== 'reply' ? target(document, prepared.action.sourceId) : null
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
        questionId: prepared.action.kind === 'retry' && source ? source.questionId : handle.turnId,
        supersedesId: prepared.action.kind === 'retry' ? (source?.id ?? null) : null,
        kind: prepared.action.kind === 'translation' ? 'translation' : (source?.kind ?? 'reply'),
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
      await cas(() =>
        this.store.compareAndSet(actor, COLLECTION, handle.conversationId, stored?.revision ?? null, checked),
      )
      return {
        handle,
        accepted: { runId, conversationId: handle.conversationId, turnId: handle.turnId },
        input,
        runInput,
      }
    })
  }
  private checkHandle(document: ConversationDocument, handle: AcceptedHandle): StoredTurn {
    if (document.summary.id !== handle.conversationId) throw notFound('turn')
    const turn = target(document, handle.turnId)
    if (turn.runId !== handle.runId || turn.reservationId !== handle.reservationId) throw notFound('turn')
    if (turn.status !== 'running') throw conflict('turn_finished')
    return turn
  }
  async finish(handle: AcceptedHandle, summary: RunSummary, actor: Actor): Promise<Conversation> {
    return this.update(async (cas) => {
      const { document, revision } = await this.load(handle.conversationId, actor),
        turn = this.checkHandle(document, handle)
      turn.status = summary.status
      turn.messages = structuredClone(summary.messages)
      turn.usage = structuredClone(summary.usage)
      turn.cost = structuredClone(summary.cost)
      turn.contextUsage = structuredClone(summary.contextUsage)
      turn.errorCode = summary.status === 'failed' ? (summary.error?.code ?? 'internal') : null
      document.summary.updatedAt = this.now()
      if (summary.status === 'completed' && turn.supersedesId === null) document.summary.selectedTurnId = turn.id
      const checked = parseDocument(document, document.summary.id)
      await cas(() => this.store.compareAndSet(actor, COLLECTION, handle.conversationId, revision, checked))
      return project(checked)
    })
  }
  /** Only the coordinator, having established interruption, may recover a planned run. */
  async recover(handle: AcceptedHandle, actor: Actor): Promise<Conversation> {
    return this.update(async (cas) => {
      const { document, revision } = await this.load(handle.conversationId, actor),
        turn = this.checkHandle(document, handle)
      turn.status = 'failed'
      turn.errorCode = 'interrupted'
      turn.contextUsage = null
      document.summary.updatedAt = this.now()
      await cas(() => this.store.compareAndSet(actor, COLLECTION, handle.conversationId, revision, document))
      return project(document)
    })
  }
  async addGenerated(handle: AcceptedHandle, images: readonly ImagePart[], actor: Actor): Promise<ImageRef[]> {
    for (const image of images) parseImage(image)
    return this.update(async (cas) => {
      const { document, revision } = await this.load(handle.conversationId, actor),
        turn = this.checkHandle(document, handle),
        first = turn.generated.length
      const now = this.now()
      turn.generated.push(...images.map((image) => ({ ...image, createdAt: now })))
      await cas(() => this.store.compareAndSet(actor, COLLECTION, handle.conversationId, revision, document))
      return imageRefs(handle.conversationId, handle.turnId, 'generated', turn.generated).slice(first)
    })
  }
  async purgeImages(beforeMs: number, actor: Actor): Promise<number> {
    if (!Number.isSafeInteger(beforeMs) || beforeMs < 0) throw badRequest('beforeMs', 'invalid')
    let total = 0
    for await (const listed of this.documents(actor)) {
      // The listed revision is tried first; after a conflict the document is reloaded, and one
      // deleted meanwhile has nothing left to purge.
      let current: StoredDocument | null = listed
      total += await this.update(async (cas) => {
        current ??= await this.store.get(actor, COLLECTION, listed.key)
        const stored = current
        current = null
        if (stored === null) return 0
        const document = parseDocument(stored.value, stored.key)
        let count = 0
        for (const turn of document.turns)
          for (const image of [...turn.images, ...turn.generated])
            if (image.data !== null && image.createdAt < beforeMs) {
              image.data = null
              count++
            }
        if (count) await cas(() => this.store.compareAndSet(actor, COLLECTION, stored.key, stored.revision, document))
        return count
      })
    }
    return total
  }
  async rename(id: string, title: string, actor: Actor): Promise<ConversationSummary> {
    checkText('title', title, 200, true)
    return this.update(async (cas) => {
      const { document, revision } = await this.load(id, actor)
      idle(document)
      document.summary.title = title.trim()
      await cas(() => this.store.compareAndSet(actor, COLLECTION, id, revision, document))
      return document.summary
    })
  }
  async delete(id: string, actor: Actor): Promise<void> {
    const { document, revision } = await this.load(id, actor)
    idle(document)
    await this.store.remove(actor, COLLECTION, id, revision)
  }
  async editTurn(id: string, turnId: string, text: string, expectedText: string, actor: Actor): Promise<Conversation> {
    checkText('text', text, 24_000, true)
    const { document, revision } = await this.load(id, actor)
    idle(document)
    const turn = target(document, turnId)
    if (document.turns.some((t) => t.parentId === turnId || t.supersedesId === turnId))
      throw conflict('turn_has_dependents')
    if (visibleText(turn) !== expectedText) throw conflict('text_changed')
    turn.messages = [{ assistant: { text: text.trim(), tool_calls: [] } }]
    turn.contextUsage = null
    document.summary.updatedAt = this.now()
    await this.store.compareAndSet(actor, COLLECTION, id, revision, document)
    return project(document)
  }
  async selectTurn(id: string, turnId: string, actor: Actor): Promise<Conversation> {
    const { document, revision } = await this.load(id, actor)
    idle(document)
    if (target(document, turnId).status !== 'completed') throw conflict('turn_not_completed')
    document.summary.selectedTurnId = turnId
    document.summary.updatedAt = this.now()
    await this.store.compareAndSet(actor, COLLECTION, id, revision, document)
    return project(document)
  }
  async removeTurn(id: string, turnId: string, actor: Actor): Promise<Conversation> {
    const { document, revision } = await this.load(id, actor)
    idle(document)
    const removed = target(document, turnId)
    const siblings = document.turns.filter(
      (t) => t.id !== turnId && t.kind === 'reply' && t.questionId === removed.questionId,
    )
    const sibling = siblings.find((t) => t.seq > removed.seq) ?? siblings.at(-1),
      successor = sibling?.id ?? removed.parentId
    const deleted = new Set(
      document.turns
        .filter((t) => t.id === turnId || (t.kind === 'translation' && t.parentId === turnId))
        .map((t) => t.id),
    )
    document.turns = document.turns.filter((t) => !deleted.has(t.id))
    for (const turn of document.turns) {
      if (turn.parentId !== null && deleted.has(turn.parentId))
        turn.parentId = successor === turn.id ? removed.parentId : successor
      if (turn.supersedesId !== null && deleted.has(turn.supersedesId))
        turn.supersedesId = sibling && sibling.id !== turn.id ? sibling.id : null
    }
    if (document.summary.selectedTurnId !== null && deleted.has(document.summary.selectedTurnId)) {
      let next =
        successor === null
          ? document.turns.findLast((t) => t.kind === 'reply' && t.parentId === null)
          : document.turns.find((t) => t.id === successor)
      document.summary.selectedTurnId = null
      const seen = new Set<string>()
      while (next) {
        if (seen.has(next.id)) throw conflict('history_cycle')
        seen.add(next.id)
        const head = next.id
        document.summary.selectedTurnId = head
        next = document.turns.findLast((t) => t.kind === 'reply' && t.parentId === head)
      }
    }
    document.summary.updatedAt = this.now()
    const checked = parseDocument(document, id)
    await this.store.compareAndSet(actor, COLLECTION, id, revision, checked)
    return project(checked)
  }
  async fork(id: string, turnId: string, actor: Actor): Promise<Conversation> {
    const { document } = await this.load(id, actor)
    idle(document)
    const path = historyPath(document, turnId),
      forkId = this.newId(),
      ids = new Map(path.map((t) => [t.id, this.newId()])),
      questions = new Map<string, string>()
    for (const turn of path)
      if (!questions.has(turn.questionId)) questions.set(turn.questionId, ids.get(turn.questionId) ?? this.newId())
    const now = this.now(),
      selected = ids.get(turnId)
    if (!selected) throw notFound('turn')
    const turns = path.map((turn, index): StoredTurn => {
      const newId = ids.get(turn.id),
        questionId = questions.get(turn.questionId)
      if (!newId || !questionId) throw conflict('history_changed')
      return {
        ...structuredClone(turn),
        id: newId,
        seq: index + 1,
        parentId: turn.parentId ? (ids.get(turn.parentId) ?? null) : null,
        questionId,
        supersedesId: turn.supersedesId ? (ids.get(turn.supersedesId) ?? null) : null,
      }
    })
    const fork: ConversationDocument = {
      version: 1,
      summary: { ...document.summary, id: forkId, selectedTurnId: selected, createdAt: now, updatedAt: now },
      turns,
    }
    await this.store.compareAndSet(actor, COLLECTION, forkId, null, fork)
    return project(fork)
  }
  async handle(request: Request, context: WorkerContext): Promise<Response | null> {
    const url = new URL(request.url),
      base = '/api/v1/conversations',
      actor = context.actor
    if (url.pathname !== base && !url.pathname.startsWith(`${base}/`)) return null
    const parts = url.pathname
      .slice(base.length)
      .split('/')
      .filter(Boolean)
      .map((part) => {
        try {
          return decodeURIComponent(part)
        } catch {
          throw badRequest('path', 'encoding')
        }
      })
    const json = (value: unknown): Response => Response.json(value)
    if (parts.length === 0 && request.method === 'GET')
      return json(await this.list(url.searchParams.get('scope'), actor))
    const id = parts[0]
    if (!id) throw notFound('route')
    if (parts.length === 1 && request.method === 'GET') return json(await this.get(id, actor))
    if (parts.length === 1 && request.method === 'DELETE') {
      await this.delete(id, actor)
      return json({ ok: true })
    }
    if (parts.length === 6 && parts[1] === 'turns' && parts[3] === 'images' && request.method === 'GET') {
      const turnId = parts[2],
        kind = parts[4],
        index = parts[5]
      if (!turnId || (kind !== 'input' && kind !== 'generated') || !index || !/^(0|[1-9]\d*)$/u.test(index))
        throw notFound('image')
      const { document } = await this.load(id, actor),
        turn = target(document, turnId),
        image = (kind === 'input' ? turn.images : turn.generated)[Number(index)]
      if (!image || image.data === null) throw notFound('image')
      const bytes = decodeImage({ mime: image.mime, data: image.data })
      return new Response(bytes, {
        headers: {
          'Content-Type': image.mime,
          'Cache-Control': 'private, no-store',
          'X-Content-Type-Options': 'nosniff',
          'Content-Security-Policy': "default-src 'none'",
        },
      })
    }
    if (parts.length !== 2 || request.method !== 'POST') throw notFound('route')
    let body: unknown
    try {
      body = await request.json()
    } catch {
      throw badRequest('body', 'json')
    }
    if (!isRecord(body)) throw badRequest('body', 'object')
    const action = parts[1],
      fields =
        action === 'rename' ? ['title'] : action === 'edit-turn' ? ['turnId', 'text', 'expectedText'] : ['turnId']
    if (Object.keys(body).some((key) => !fields.includes(key))) throw badRequest('body', 'unknown_field')
    if (action === 'rename') {
      if (typeof body.title !== 'string') throw badRequest('title', 'required')
      return json(await this.rename(id, body.title, actor))
    }
    if (typeof body.turnId !== 'string') throw badRequest('turnId', 'required')
    if (action === 'edit-turn') {
      if (typeof body.text !== 'string' || typeof body.expectedText !== 'string') throw badRequest('text', 'required')
      return json(await this.editTurn(id, body.turnId, body.text, body.expectedText, actor))
    }
    if (action === 'remove-turn') return json(await this.removeTurn(id, body.turnId, actor))
    if (action === 'select-turn') return json(await this.selectTurn(id, body.turnId, actor))
    if (action === 'fork') return json(await this.fork(id, body.turnId, actor))
    throw notFound('route')
  }
}
