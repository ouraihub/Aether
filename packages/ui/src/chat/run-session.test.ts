import { describe, expect, it, vi } from 'vitest'
import { createFakePorts, KitClientError } from '../ports'
import type { ConversationClient, Conversation, RunAccepted, RunEvent, RunRequest, Turn } from '../ports'
import { RunSession, type RunBase } from './run-session.svelte'

const sourceModel = { endpointId: 'original-endpoint', modelId: 'original-model' }
const pickedModel = { endpointId: 'picked-endpoint', modelId: 'picked-model' }
const base: RunBase = {
  purpose: 'chat',
  scope: null,
  language: 'en',
  model: pickedModel,
  skills: [],
  context: null,
}

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected run-session fixture is missing')
  return value
}

function stored(overrides: Partial<Turn> = {}): Turn {
  return {
    id: 'source',
    parentId: null,
    questionId: 'question',
    supersedesId: null,
    kind: 'reply',
    endpointId: sourceModel.endpointId,
    modelId: sourceModel.modelId,
    input: 'Original question',
    text: 'Original **answer**',
    status: 'completed',
    errorCode: null,
    reasoningText: 'Original reasoning',
    reasoningEffort: 'default',
    images: [],
    generated: [],
    ...overrides,
  }
}

function conversation(turns: Turn[], selectedTurnId: string | null = 'source'): Conversation {
  return {
    summary: { id: 'conversation', title: 'Conversation', scope: null, createdAt: 0, updatedAt: 0, selectedTurnId },
    turns,
  }
}

interface Deferred<T> {
  promise: Promise<T>
  resolve: (value: T) => void
  reject: (error: unknown) => void
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

function setup(withHistory = true) {
  const runs: { request: RunRequest; emit: (event: RunEvent) => void; accepted: Deferred<RunAccepted> }[] = []
  const get = vi.fn<ConversationClient['get']>(() => Promise.resolve(conversation([stored()])))
  const cancel = vi.fn(() => Promise.resolve())
  const approve = vi.fn(() => Promise.resolve())
  const ports = createFakePorts({
    chat: {
      cancel,
      approve,
      start: (request, emit) => {
        const accepted = deferred<RunAccepted>()
        runs.push({ request, emit, accepted })
        return accepted.promise
      },
    },
  })
  const chat = required(ports.chat)
  if (withHistory) Object.assign(required(ports.conversations), { get })
  else delete ports.conversations
  return { session: new RunSession(chat, ports.conversations), runs, get, cancel, approve }
}

const accepted: RunAccepted = { runId: 'run', conversationId: 'conversation', turnId: 'candidate' }

describe('RunSession candidate replies', () => {
  it('restores graph metadata and sends against the selected parent, not the last candidate', async () => {
    const { session, runs } = setup()
    const source = stored()
    const sibling = stored({ id: 'sibling', supersedesId: source.id })
    session.show('conversation', [source, sibling], source.id)
    expect(session.turns[0]).toMatchObject({
      id: source.id,
      text: source.text,
      questionId: source.questionId,
      ...sourceModel,
      error: source.errorCode,
      persisted: true,
    })
    expect(session.selectedTurnId).toBe(source.id)
    const sent = session.send(base, 'Next question')
    expect(required(runs[0]).request).toMatchObject({ replyToId: source.id, regenerate: false })
    expect(session.turns[2]).toMatchObject({ parentId: source.id, kind: 'reply', ...pickedModel })
    required(runs[0]).accepted.resolve(accepted)
    expect(await sent).toBe(true)
    expect(session.turns[2]).toMatchObject({
      id: 'candidate',
      questionId: 'candidate',
      parentId: source.id,
      persisted: true,
    })
  })

  it('keeps the source and its question when retrying with its original model snapshot', async () => {
    const { session, runs, get } = setup()
    const source = stored({ parentId: 'parent' })
    const candidate = stored({ id: 'candidate', parentId: 'parent', supersedesId: source.id, text: 'New answer' })
    session.show('conversation', [source], source.id)
    get.mockResolvedValue(conversation([source, candidate]))
    const retried = session.retry(base, source.id)
    expect(required(runs[0]).request).toMatchObject({
      retryTurnId: source.id,
      model: sourceModel,
      input: source.input,
      regenerate: false,
    })
    expect(required(runs[0]).request.replyToId).toBeUndefined()
    expect(session.turns[0]).toMatchObject({
      id: source.id,
      text: source.text,
      reasoningText: source.reasoningText,
      questionId: source.questionId,
    })
    expect(session.turns[1]).toMatchObject({
      questionId: source.questionId,
      parentId: source.parentId,
      supersedesId: source.id,
    })
    required(runs[0]).accepted.resolve(accepted)
    await retried
    required(runs[0]).emit({ kind: 'text_delta', text: 'New answer' })
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(session.turns).toHaveLength(2)
    expect(required(session.turns[0]).text).toBe(source.text)
    expect(session.selectedTurnId).toBe(source.id)
  })

  it('honors an explicit switch model while keeping the question and source reasoning effort', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored({ reasoningEffort: 'high' })], 'source')
    const retried = session.retry(base, 'source', pickedModel)
    expect(required(runs[0]).request).toMatchObject({
      model: pickedModel,
      reasoningEffort: 'high',
      retryTurnId: 'source',
    })
    required(runs[0]).accepted.resolve(accepted)
    await retried
    expect(session.turns[1]).toMatchObject({ id: 'candidate', questionId: 'question', ...pickedModel })
  })

  it('translates the original reply with the real instruction and original model', async () => {
    const { session, runs } = setup()
    const source = stored()
    session.show('conversation', [source], source.id)
    const translated = session.translate({ ...base, model: null }, source.id, ' French ')
    const instruction = `Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\n${source.text}`
    expect(required(runs[0]).request).toMatchObject({
      translateTurnId: source.id,
      targetLanguage: 'French',
      model: sourceModel,
      input: instruction,
    })
    expect(session.turns[1]).toMatchObject({
      kind: 'translation',
      parentId: source.id,
      supersedesId: null,
      input: instruction,
    })
    expect(required(session.turns[0]).text).toBe(source.text)
    required(runs[0]).accepted.resolve(accepted)
    await translated
    expect(required(session.turns[1]).questionId).toBe('candidate')
  })

  it('honors the translation dialog model selection', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    const translated = session.translate(base, 'source', 'French')
    expect(required(runs[0]).request.model).toEqual(pickedModel)
    required(runs[0]).accepted.resolve(accepted)
    await translated
    expect(session.turns[1]).toMatchObject(pickedModel)
  })

  it('preserves translation kind when retrying a translated reply', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored({ kind: 'translation' })], 'source')
    const retried = session.retry(base, 'source')
    expect(session.turns[1]).toMatchObject({ kind: 'translation', questionId: 'question', supersedesId: 'source' })
    required(runs[0]).accepted.resolve(accepted)
    await retried
  })

  it('waits for acceptance after an early end, then merges stored text and metadata without losing tools/search', async () => {
    const { session, runs, get } = setup()
    const source = stored()
    const candidate = stored({
      id: 'candidate',
      questionId: 'candidate',
      parentId: source.id,
      text: 'Persisted answer',
    })
    session.show('conversation', [source], source.id)
    get.mockResolvedValue(conversation([source, candidate], candidate.id))
    const sent = session.send(base, 'Next question', true)
    const run = required(runs[0])
    run.emit({ kind: 'text_delta', text: 'Streamed answer' })
    run.emit({ kind: 'tool_call', callId: 'tool', name: 'read_note', source: 'host' })
    run.emit({ kind: 'tool_result', callId: 'tool', outcome: 'ok', summary: 'Read note' })
    const hits = [{ title: 'Source', url: 'https://example.com', snippet: 'Excerpt' }]
    run.emit({ kind: 'search_result', query: 'Next question', provider: 'search-provider', hits })
    run.emit({ kind: 'end', status: 'completed', exhausted: true })
    expect(get).not.toHaveBeenCalled()
    expect(session.running).toBe(true)
    run.accepted.resolve(accepted)
    await sent
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(get).toHaveBeenCalledOnce()
    expect(get).toHaveBeenCalledWith('conversation')
    expect(session.turns[1]).toMatchObject({
      id: candidate.id,
      text: candidate.text,
      questionId: candidate.questionId,
      parentId: candidate.parentId,
      exhausted: true,
      tools: [{ callId: 'tool', summary: 'Read note', outcome: 'ok' }],
      search: { status: 'complete', hits },
    })
    expect(session.selectedTurnId).toBe(candidate.id)
    session.show('conversation', [source, candidate], source.id)
    expect(required(required(session.turns[1]).tools[0]).summary).toBe('Read note')
    expect(required(session.turns[1]).search.hits).toEqual(hits)
    session.show(
      'conversation',
      [source, { ...candidate, text: 'Edited answer', reasoningText: null, contextUsage: null }],
      source.id,
    )
    expect(required(session.turns[1]).tools).toEqual([])
    expect(required(session.turns[1]).search.status).toBe('idle')
  })

  it('retains originals and exposes both acceptance rejection and streamed failure', async () => {
    const { session, runs, get } = setup()
    const source = stored()
    session.show('conversation', [source], source.id)
    const retry = session.retry(base, source.id)
    required(runs[0]).accepted.reject(new KitClientError({ code: 'needs_config', message: 'Source model removed' }))
    expect(await retry).toBe(false)
    expect(session.actionError).toBe('needs_config')
    expect(session.turns[0]).toMatchObject({ id: source.id, text: source.text, questionId: source.questionId })
    expect(session.turns).toHaveLength(1)
    expect(session.selectedTurnId).toBe(source.id)
    expect(get).not.toHaveBeenCalled()

    get.mockResolvedValue(conversation([source]))
    const translation = session.translate(base, source.id, 'French')
    expect(session.actionError).toBeNull()
    required(runs[1]).accepted.resolve(accepted)
    await translation
    required(runs[1]).emit({ kind: 'end', status: 'failed', error: { code: 'upstream', message: 'Provider failed' } })
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(session.actionError).toBeNull()
    expect(session.turns[1]).toMatchObject({ status: 'failed', error: 'upstream' })
    expect(session.turns[0]).toMatchObject({ id: source.id, text: source.text, questionId: source.questionId })
    expect(session.selectedTurnId).toBe(source.id)
  })

  it('removes a rejected translation candidate and retains its original', async () => {
    const { session, runs } = setup()
    const source = stored()
    session.show('conversation', [source], source.id)
    const translated = session.translate(base, source.id, 'French')
    required(runs[0]).accepted.reject(new KitClientError({ code: 'upstream', message: 'Provider unavailable' }))
    expect(await translated).toBe(false)
    expect(session.actionError).toBe('upstream')
    expect(session.turns).toHaveLength(1)
    expect(required(session.turns[0]).text).toBe(source.text)
    expect(session.selectedTurnId).toBe(source.id)
  })

  it('keeps the streamed reply and exposes a failed persistence refresh', async () => {
    const { session, runs, get } = setup()
    session.show('conversation', [stored()], 'source')
    get.mockRejectedValue(new Error('History unavailable'))
    const retry = session.retry(base, 'source')
    required(runs[0]).accepted.resolve(accepted)
    await retry
    required(runs[0]).emit({ kind: 'text_delta', text: 'Still visible' })
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(session.turns[1]).toMatchObject({ id: 'candidate', text: 'Still visible' })
    expect(session.actionError).toBeNull()
    expect(session.historyRefreshError).toBe('History unavailable')
  })

  it('retains a rejected normal reply locally but never marks its temporary id persisted', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    const sent = session.send(base, 'New question')
    required(runs[0]).accepted.reject(new KitClientError({ code: 'upstream', message: 'Unavailable' }))
    expect(await sent).toBe(false)
    expect(session.turns[1]).toMatchObject({ persisted: false, status: 'failed' })
    expect(session.selectedTurnId).toBe('source')
    expect(session.actionError).toBeNull()
    const localId = required(session.turns[1]).id
    const retry = session.retry(base, localId)
    expect(required(runs[1]).request).toMatchObject({ replyToId: 'source', input: 'New question', regenerate: false })
    expect(required(runs[1]).request.retryTurnId).toBeUndefined()
    required(runs[1]).accepted.resolve(accepted)
    await retry
    expect(session.turns[2]).toMatchObject({ id: 'candidate', questionId: 'candidate', persisted: true })
  })

  it('ignores delayed acceptance and history refresh when another conversation is shown', async () => {
    const { session, runs, get } = setup()
    const pendingHistory = deferred<Conversation>()
    get.mockReturnValue(pendingHistory.promise)
    session.show('conversation', [stored()], 'source')
    const sent = session.send(base, 'Next')
    required(runs[0]).accepted.resolve(accepted)
    await sent
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    session.show('other', [stored({ id: 'other-source' })], 'other-source')
    pendingHistory.resolve(conversation([stored()]))
    await pendingHistory.promise
    expect(session.conversationId).toBe('other')
    expect(required(session.turns[0]).id).toBe('other-source')

    const delayed = session.send(base, 'Delayed')
    session.show(null)
    required(runs[1]).accepted.resolve(accepted)
    await delayed
    expect(session.conversationId).toBeNull()
    expect(session.turns).toEqual([])
  })

  it('without history appends fresh retry/translation candidates without backend graph fields', async () => {
    const { session, runs } = setup(false)
    const source = stored()
    session.show(null, [source], source.id)
    const retry = session.retry(base, source.id)
    expect(required(runs[0]).request).toMatchObject({
      input: source.input,
      regenerate: false,
      conversationId: null,
      model: sourceModel,
    })
    expect(required(runs[0]).request.retryTurnId).toBeUndefined()
    expect(required(runs[0]).request.replyToId).toBeUndefined()
    required(runs[0]).accepted.resolve({ runId: 'run', conversationId: null, turnId: null })
    await retry
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    expect(session.turns).toHaveLength(2)
    expect(required(session.turns[0]).text).toBe(source.text)
    expect(session.selectedTurnId).toBe(source.id)
    const translation = session.translate(base, source.id, 'French')
    expect(required(runs[1]).request.translateTurnId).toBeUndefined()
    expect(required(runs[1]).request.targetLanguage).toBeUndefined()
    expect(required(runs[1]).request.input).toContain(source.text)
    required(runs[1]).accepted.resolve({ runId: 'translated', conversationId: null, turnId: null })
    await translation
    expect(session.turns).toHaveLength(3)
  })

  it('rejects reply actions for writing and omits graph fields from a normal writing run', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    const writing = { ...base, purpose: 'writing' }
    expect(await session.retry(writing, 'source')).toBe(false)
    expect(session.actionError).toBe('bad_request')
    expect(await session.translate(writing, 'source', 'French')).toBe(false)
    expect(runs).toHaveLength(0)
    const sent = session.send(writing, 'Polish this')
    expect(required(runs[0]).request.replyToId).toBeUndefined()
    expect(required(runs[0]).request.retryTurnId).toBeUndefined()
    expect(required(runs[0]).request.translateTurnId).toBeUndefined()
    required(runs[0]).accepted.resolve(accepted)
    await sent
  })

  it('uses the picked model for an unaccepted local reply without a source model snapshot', async () => {
    const { session, runs } = setup(false)
    const sent = session.send({ ...base, model: null }, 'Question')
    required(runs[0]).accepted.reject(new KitClientError({ code: 'upstream', message: 'Unavailable' }))
    await sent
    const retried = session.retry(base, required(session.turns[0]).id)
    expect(required(runs[1]).request.model).toEqual(pickedModel)
    required(runs[1]).accepted.resolve({ runId: 'local', conversationId: null, turnId: null })
    await retried
  })

  it('rejects missing source and invalid language without changing original replies', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    expect(await session.retry(base, 'missing')).toBe(false)
    expect(session.actionError).toBe('not_found')
    expect(await session.translate(base, 'source', ' ')).toBe(false)
    expect(session.actionError).toBe('bad_request')
    expect(await session.translate(base, 'source', 'x'.repeat(81))).toBe(false)
    expect(session.turns).toHaveLength(1)
    expect(runs).toHaveLength(0)
  })

  it('shows sent images at once, omits them when empty and lets history replace them', async () => {
    const { session, runs, get } = setup()
    session.show('conversation', [stored()], 'source')
    void session.send(base, 'Plain')
    expect('images' in required(runs[0]).request).toBe(false)
    required(runs[0]).accepted.reject(new KitClientError({ code: 'network', message: 'offline' }))
    await vi.waitFor(() => expect(session.running).toBe(false))
    const sent = session.send(base, 'Look', false, [{ mime: 'image/png', data: 'AAAA' }])
    expect(required(runs[1]).request.images).toEqual([{ mime: 'image/png', data: 'AAAA' }])
    expect(session.turns.at(-1)?.images).toEqual([{ mime: 'image/png', url: 'data:image/png;base64,AAAA' }])
    const hosted = { mime: 'image/png', url: '/images/0' }
    get.mockResolvedValue(conversation([stored(), stored({ id: 'candidate', images: [hosted] })]))
    required(runs[1]).accepted.resolve(accepted)
    await sent
    required(runs[1]).emit({ kind: 'end', status: 'completed' })
    await vi.waitFor(() => expect(session.turns.find((turn) => turn.id === 'candidate')?.images).toEqual([hosted]))
  })

  it('retries a stored source by id only and an unstored one by resending its images', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored({ images: [{ mime: 'image/png', url: '/images/0' }] })], 'source')
    void session.retry(base, 'source')
    expect(required(runs[0]).request).toMatchObject({ retryTurnId: 'source' })
    expect('images' in required(runs[0]).request).toBe(false)
    expect(session.turns[1]?.images).toEqual([{ mime: 'image/png', url: '/images/0' }])

    const local = setup(false)
    const sent = local.session.send(base, 'Look', false, [{ mime: 'image/gif', data: 'R0lG' }])
    required(local.runs[0]).accepted.reject(new KitClientError({ code: 'upstream', message: 'Unavailable' }))
    await sent
    void local.session.retry(base, required(local.session.turns[0]).id)
    expect(required(local.runs[1]).request.images).toEqual([{ mime: 'image/gif', data: 'R0lG' }])
  })

  it('takes the streamed cost and sums known costs per currency', async () => {
    const { session, runs } = setup(false)
    session.show(null, [stored({ cost: { amount: 0.5, currency: 'USD' } }), stored({ id: 'b', cost: null })], 'b')
    void session.send(base, 'Priced')
    const usage = { inputTokens: 10, outputTokens: 2, cacheReadTokens: null, cacheWriteTokens: null }
    required(runs[0]).emit({ kind: 'usage', ...usage, reasoningTokens: null, cost: { amount: 1, currency: 'CNY' } })
    expect(session.turns.at(-1)?.cost).toEqual({ amount: 1, currency: 'CNY' })
    session.turns.push({ ...required(session.turns.at(-1)), id: 'c', cost: { amount: 0.25, currency: 'USD' } })
    expect(session.cumulativeCost).toEqual([
      { amount: 0.75, currency: 'USD' },
      { amount: 1, currency: 'CNY' },
    ])
  })

  it('does not invent a terminal state when cancellation is acknowledged', async () => {
    const { session, runs, cancel } = setup(false)
    const sent = session.send(base, 'Partial')
    const run = required(runs[0])
    run.accepted.resolve({ runId: 'r', conversationId: null, turnId: null })
    await sent
    run.emit({ kind: 'text_delta', text: 'kept' })
    session.stop()
    await Promise.resolve()
    expect(cancel).toHaveBeenCalledWith('r')
    expect(session.running).toBe(true)
    expect(session.turns[0]).toMatchObject({ text: 'kept', status: 'running' })
    run.emit({ kind: 'end', status: 'cancelled' })
    expect(session.running).toBe(false)
    expect(session.turns[0]).toMatchObject({ text: 'kept', status: 'cancelled' })
  })

  it('cancels late acceptance and isolates old events after changing the displayed conversation', async () => {
    const { session, runs, cancel } = setup(false)
    const sent = session.send(base, 'Old')
    const old = required(runs[0])
    session.show('new', [stored({ id: 'new-turn', text: 'New' })])
    old.emit({ kind: 'text_delta', text: 'old text' })
    old.accepted.resolve({ runId: 'late', conversationId: 'old', turnId: 'old-turn' })
    await sent
    old.emit({ kind: 'end', status: 'completed' })
    expect(cancel).toHaveBeenCalledWith('late')
    expect(session.conversationId).toBe('new')
    expect(session.turns).toHaveLength(1)
    expect(session.turns[0]).toMatchObject({ id: 'new-turn', text: 'New' })
  })

  it('reports a current approval failure but does not revive a card after switching sessions', async () => {
    const { session, runs, approve } = setup(false)
    const failure = deferred<void>()
    approve.mockReturnValueOnce(failure.promise)
    const sent = session.send(base, 'Tool')
    const run = required(runs[0])
    run.accepted.resolve({ runId: 'r', conversationId: null, turnId: null })
    await sent
    run.emit({ kind: 'tool_call', callId: 'call', name: 'tool', source: 'host' })
    run.emit({ kind: 'approval_required', callId: 'call', name: 'tool', argsPreview: 'preview' })
    const tool = required(session.turns[0]?.tools[0])
    session.approve(tool, true)
    failure.reject(new KitClientError({ code: 'conflict', message: 'expired' }))
    await Promise.resolve()
    expect(session.actionError).toBe('conflict')
    expect(tool.pending).toBe('preview')
    const oldFailure = deferred<void>()
    approve.mockReturnValueOnce(oldFailure.promise)
    session.approve(tool, true)
    session.show(null)
    oldFailure.reject(new KitClientError({ code: 'not_found', message: 'old' }))
    await Promise.resolve()
    expect(session.turns).toEqual([])
    expect(session.actionError).toBeNull()
    expect(tool.pending).toBeNull()
  })
})
