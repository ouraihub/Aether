import type {
  Conversation,
  ConversationSummary,
  ImagePart,
  ImageRef,
} from '@ouraihub/aether-contracts'
import type { RunSummary } from '@ouraihub/aether-core/types'
import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import { isRecord } from '@ouraihub/aether-core/guard'
import { decodeImage, parseImage } from '@ouraihub/aether-core/images'
import type { Actor, WorkerContext } from '../context.ts'
import { ConversationService as ConversationCore } from './core.ts'
import {
  COLLECTION,
  checkText,
  conflict,
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
import type { AcceptedHandle } from './core.ts'
export class ConversationService extends ConversationCore {
  private checkHandle(
    document: ConversationDocument,
    handle: AcceptedHandle,
  ): StoredTurn {
    if (document.summary.id !== handle.conversationId) throw notFound('turn')
    const turn = target(document, handle.turnId)
    if (
      turn.runId !== handle.runId ||
      turn.reservationId !== handle.reservationId
    )
      throw notFound('turn')
    if (turn.status !== 'running') throw conflict('turn_finished')
    return turn
  }
  async finish(
    handle: AcceptedHandle,
    summary: RunSummary,
    actor: Actor,
  ): Promise<Conversation> {
    for (let attempt = 0; attempt < 8; attempt++) {
      const { document, revision } = await this.load(
          handle.conversationId,
          actor,
        ),
        turn = this.checkHandle(document, handle)
      turn.status = summary.status
      turn.messages = structuredClone(summary.messages)
      turn.usage = structuredClone(summary.usage)
      turn.cost = structuredClone(summary.cost)
      turn.contextUsage = structuredClone(summary.contextUsage)
      turn.errorCode =
        summary.status === 'failed' ? (summary.error?.code ?? 'internal') : null
      document.summary.updatedAt = this.now()
      if (summary.status === 'completed' && turn.supersedesId === null)
        document.summary.selectedTurnId = turn.id
      const checked = parseDocument(document, document.summary.id)
      try {
        await this.store.compareAndSet(
          actor,
          COLLECTION,
          handle.conversationId,
          revision,
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
      return project(checked)
    }
    throw conflict('conversation_contended')
  }
  /** Only the coordinator, having established interruption, may recover a planned run. */
  async recover(handle: AcceptedHandle, actor: Actor): Promise<Conversation> {
    for (let attempt = 0; attempt < 8; attempt++) {
      const { document, revision } = await this.load(
          handle.conversationId,
          actor,
        ),
        turn = this.checkHandle(document, handle)
      turn.status = 'failed'
      turn.errorCode = 'interrupted'
      turn.contextUsage = null
      document.summary.updatedAt = this.now()
      try {
        await this.store.compareAndSet(
          actor,
          COLLECTION,
          handle.conversationId,
          revision,
          document,
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
      return project(document)
    }
    throw conflict('conversation_contended')
  }
  async addGenerated(
    handle: AcceptedHandle,
    images: readonly ImagePart[],
    actor: Actor,
  ): Promise<ImageRef[]> {
    for (const image of images) parseImage(image)
    for (let attempt = 0; attempt < 8; attempt++) {
      const { document, revision } = await this.load(
          handle.conversationId,
          actor,
        ),
        turn = this.checkHandle(document, handle),
        first = turn.generated.length
      const now = this.now()
      turn.generated.push(
        ...images.map((image) => ({ ...image, createdAt: now })),
      )
      try {
        await this.store.compareAndSet(
          actor,
          COLLECTION,
          handle.conversationId,
          revision,
          document,
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
      return imageRefs(
        handle.conversationId,
        handle.turnId,
        'generated',
        turn.generated,
      ).slice(first)
    }
    throw conflict('conversation_contended')
  }
  async purgeImages(beforeMs: number, actor: Actor): Promise<number> {
    if (!Number.isSafeInteger(beforeMs) || beforeMs < 0)
      throw badRequest('beforeMs', 'invalid')
    let total = 0
    for await (const stored of this.documents(actor)) {
      const document = parseDocument(stored.value, stored.key)
      let count = 0
      for (const turn of document.turns)
        for (const image of [...turn.images, ...turn.generated])
          if (image.data !== null && image.createdAt < beforeMs) {
            image.data = null
            count++
          }
      if (count) {
        await this.store.compareAndSet(
          actor,
          COLLECTION,
          stored.key,
          stored.revision,
          document,
        )
        total += count
      }
    }
    return total
  }
  async rename(
    id: string,
    title: string,
    actor: Actor,
  ): Promise<ConversationSummary> {
    checkText('title', title, 200, true)
    const { document, revision } = await this.load(id, actor)
    idle(document)
    document.summary.title = title.trim()
    await this.store.compareAndSet(actor, COLLECTION, id, revision, document)
    return document.summary
  }
  async delete(id: string, actor: Actor): Promise<void> {
    const { document, revision } = await this.load(id, actor)
    idle(document)
    await this.store.remove(actor, COLLECTION, id, revision)
  }
  async editTurn(
    id: string,
    turnId: string,
    text: string,
    expectedText: string,
    actor: Actor,
  ): Promise<Conversation> {
    checkText('text', text, 24_000, true)
    const { document, revision } = await this.load(id, actor)
    idle(document)
    const turn = target(document, turnId)
    if (
      document.turns.some(
        (t) => t.parentId === turnId || t.supersedesId === turnId,
      )
    )
      throw conflict('turn_has_dependents')
    if (visibleText(turn) !== expectedText) throw conflict('text_changed')
    turn.messages = [{ assistant: { text: text.trim(), tool_calls: [] } }]
    turn.contextUsage = null
    document.summary.updatedAt = this.now()
    await this.store.compareAndSet(actor, COLLECTION, id, revision, document)
    return project(document)
  }
  async selectTurn(
    id: string,
    turnId: string,
    actor: Actor,
  ): Promise<Conversation> {
    const { document, revision } = await this.load(id, actor)
    idle(document)
    if (target(document, turnId).status !== 'completed')
      throw conflict('turn_not_completed')
    document.summary.selectedTurnId = turnId
    document.summary.updatedAt = this.now()
    await this.store.compareAndSet(actor, COLLECTION, id, revision, document)
    return project(document)
  }
  async removeTurn(
    id: string,
    turnId: string,
    actor: Actor,
  ): Promise<Conversation> {
    const { document, revision } = await this.load(id, actor)
    idle(document)
    const removed = target(document, turnId)
    const siblings = document.turns.filter(
      (t) =>
        t.id !== turnId &&
        t.kind === 'reply' &&
        t.questionId === removed.questionId,
    )
    const sibling =
        siblings.find((t) => t.seq > removed.seq) ?? siblings.at(-1),
      successor = sibling?.id ?? removed.parentId
    const deleted = new Set(
      document.turns
        .filter(
          (t) =>
            t.id === turnId ||
            (t.kind === 'translation' && t.parentId === turnId),
        )
        .map((t) => t.id),
    )
    document.turns = document.turns.filter((t) => !deleted.has(t.id))
    for (const turn of document.turns) {
      if (turn.parentId !== null && deleted.has(turn.parentId))
        turn.parentId = successor === turn.id ? removed.parentId : successor
      if (turn.supersedesId !== null && deleted.has(turn.supersedesId))
        turn.supersedesId =
          sibling && sibling.id !== turn.id ? sibling.id : null
    }
    if (
      document.summary.selectedTurnId !== null &&
      deleted.has(document.summary.selectedTurnId)
    ) {
      let next =
        successor === null
          ? document.turns.findLast(
              (t) => t.kind === 'reply' && t.parentId === null,
            )
          : document.turns.find((t) => t.id === successor)
      document.summary.selectedTurnId = null
      const seen = new Set<string>()
      while (next) {
        if (seen.has(next.id)) throw conflict('history_cycle')
        seen.add(next.id)
        const head = next.id
        document.summary.selectedTurnId = head
        next = document.turns.findLast(
          (t) => t.kind === 'reply' && t.parentId === head,
        )
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
      if (!questions.has(turn.questionId))
        questions.set(turn.questionId, ids.get(turn.questionId) ?? this.newId())
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
        supersedesId: turn.supersedesId
          ? (ids.get(turn.supersedesId) ?? null)
          : null,
      }
    })
    const fork: ConversationDocument = {
      version: 1,
      summary: {
        ...document.summary,
        id: forkId,
        selectedTurnId: selected,
        createdAt: now,
        updatedAt: now,
      },
      turns,
    }
    await this.store.compareAndSet(actor, COLLECTION, forkId, null, fork)
    return project(fork)
  }
  async handle(
    request: Request,
    context: WorkerContext,
  ): Promise<Response | null> {
    const url = new URL(request.url),
      base = '/api/v1/conversations',
      actor = context.actor
    if (url.pathname !== base && !url.pathname.startsWith(`${base}/`))
      return null
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
    if (parts.length === 1 && request.method === 'GET')
      return json(await this.get(id, actor))
    if (parts.length === 1 && request.method === 'DELETE') {
      await this.delete(id, actor)
      return json({ ok: true })
    }
    if (
      parts.length === 6 &&
      parts[1] === 'turns' &&
      parts[3] === 'images' &&
      request.method === 'GET'
    ) {
      const turnId = parts[2],
        kind = parts[4],
        index = parts[5]
      if (
        !turnId ||
        (kind !== 'input' && kind !== 'generated') ||
        !index ||
        !/^(0|[1-9]\d*)$/u.test(index)
      )
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
        action === 'rename'
          ? ['title']
          : action === 'edit-turn'
            ? ['turnId', 'text', 'expectedText']
            : ['turnId']
    if (Object.keys(body).some((key) => !fields.includes(key)))
      throw badRequest('body', 'unknown_field')
    if (action === 'rename') {
      if (typeof body.title !== 'string') throw badRequest('title', 'required')
      return json(await this.rename(id, body.title, actor))
    }
    if (typeof body.turnId !== 'string') throw badRequest('turnId', 'required')
    if (action === 'edit-turn') {
      if (
        typeof body.text !== 'string' ||
        typeof body.expectedText !== 'string'
      )
        throw badRequest('text', 'required')
      return json(
        await this.editTurn(
          id,
          body.turnId,
          body.text,
          body.expectedText,
          actor,
        ),
      )
    }
    if (action === 'remove-turn')
      return json(await this.removeTurn(id, body.turnId, actor))
    if (action === 'select-turn')
      return json(await this.selectTurn(id, body.turnId, actor))
    if (action === 'fork') return json(await this.fork(id, body.turnId, actor))
    throw notFound('route')
  }
}
