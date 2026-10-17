import { describe, expect, it } from 'vitest'
import { createFakePorts, isKitClientError } from './index'
import type { Conversation, RunEvent, RunRequest, Turn } from './index'

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected fixture is missing')
  return value
}

const model = { endpointId: 'endpoint', modelId: 'model' }
const request: RunRequest = { purpose: 'chat', skills: [], input: 'Question', regenerate: false, language: 'en', model }
const turn = (id: string, overrides: Partial<Turn> = {}): Turn => ({
  id,
  parentId: null,
  questionId: id,
  supersedesId: null,
  kind: 'reply',
  ...model,
  status: 'completed',
  input: 'Question',
  text: 'Original answer',
  errorCode: null,
  reasoningText: 'Thinking',
  images: [],
  generated: [],
  usage: { inputTokens: 100, outputTokens: 60, cacheReadTokens: 20, cacheWriteTokens: null, reasoningTokens: 4 },
  contextUsage: { tokens: 160, contextWindow: 1000, source: 'reported', ...model },
  ...overrides,
})
const stored = (turns: Turn[], selectedTurnId: string | null = turns[0]?.id ?? null): Conversation => ({
  summary: { id: 'stored', title: 'Stored', scope: 'scope', createdAt: 1, updatedAt: 2, selectedTurnId },
  turns,
})

async function code(promise: Promise<unknown>) {
  const error: unknown = await promise.catch((caught: unknown) => caught)
  return isKitClientError(error) ? error.code : null
}

describe('createFakePorts', () => {
  it('declares every port by default', () => {
    expect(Object.keys(createFakePorts()).sort()).toEqual(
      ['chat', 'logs', 'mcpClient', 'mcpServer', 'providers', 'skills'].sort(),
    )
  })

  it('omits ports like a host without the feature', () => {
    const ports = createFakePorts({ omit: ['chat', 'mcpServer'] })
    expect(ports.chat).toBeUndefined()
    expect(ports.mcpServer).toBeUndefined()
    expect(ports.providers).toBeDefined()
  })

  it('overrides single methods and keeps the rest', async () => {
    const ports = createFakePorts({
      providers: { list: () => Promise.resolve([{ id: 'p', name: 'P', enabled: true, notes: '', endpoints: [] }]) },
    })
    expect(await ports.providers?.list()).toEqual([{ id: 'p', name: 'P', enabled: true, notes: '', endpoints: [] }])
    expect(await ports.providers?.modelOptions()).toEqual([])
  })

  it('accepts stable stored IDs and persists before emitting the single end', async () => {
    const chat = required(createFakePorts().chat)
    const events: RunEvent[] = []
    let persisted: Promise<Conversation> | undefined
    const run = chat.run(request, (event) => {
      events.push(event)
      if (event.kind === 'end') persisted = required(chat.history).get('fake-conversation-1')
    })
    expect(events).toEqual([])
    const accepted = await run
    expect(accepted).toMatchObject({ conversationId: 'fake-conversation-1', turnId: 'fake-turn-1' })
    expect((await persisted)?.turns[0]).toMatchObject({
      id: accepted.turnId,
      status: 'completed',
      text: 'Fake reply: Question',
    })
    expect(events).toEqual([
      { kind: 'text_delta', text: 'Fake reply: Question' },
      { kind: 'end', status: 'completed' },
    ])
    expect((await required(chat.history).list())[0]?.selectedTurnId).toBe(accepted.turnId)
  })
  it('lists scoped storage, renames and removes real conversations', async () => {
    const history = required(createFakePorts({ conversations: [stored([turn('source')])] }).chat?.history)
    expect(await history.list('other')).toEqual([])
    expect((await history.list('scope'))[0]?.id).toBe('stored')
    expect((await history.rename('stored', 'Renamed')).title).toBe('Renamed')
    const listed = await history.list()
    required(listed[0]).title = 'External mutation'
    expect((await history.get('stored')).summary.title).toBe('Renamed')
    await history.remove('stored')
    expect(await history.list()).toEqual([])
    expect(await code(history.get('stored'))).toBe('not_found')
  })

  it('rejects cyclic and missing fork ancestors without creating partial conversations', async () => {
    for (const parentId of ['source', 'missing']) {
      const history = required(
        createFakePorts({ conversations: [stored([turn('source', { parentId })])] }).chat?.history,
      )
      expect(await code(history.fork('stored', 'source'))).toBe(parentId === 'source' ? 'conflict' : 'not_found')
      expect(await history.list()).toHaveLength(1)
    }
  })

  it('retains end-only no-store demos and rejects already aborted runs', async () => {
    const chat = required(createFakePorts().chat)
    delete chat.history
    const events: RunEvent[] = []
    expect(await chat.run(request, (event) => events.push(event))).toEqual({
      runId: 'fake-run',
      conversationId: null,
      turnId: null,
    })
    expect(events).toEqual([{ kind: 'end', status: 'completed' }])
    expect(await code(chat.run(request, () => undefined, { signal: AbortSignal.abort() }))).toBe('cancelled')
  })

  it('preserves candidate question, parent and source model without selecting retry', async () => {
    const chat = required(
      createFakePorts({
        conversations: [stored([turn('parent'), turn('source', { parentId: 'parent' })], 'source')],
      }).chat,
    )
    const accepted = await chat.run(
      { ...request, model: null, conversationId: 'stored', retryTurnId: 'source', input: 'Ignored' },
      () => undefined,
    )
    const saved = await required(chat.history).get('stored')
    expect(saved.turns[2]).toMatchObject({
      id: accepted.turnId,
      parentId: 'parent',
      questionId: 'source',
      supersedesId: 'source',
      input: 'Question',
      ...model,
    })
    expect(saved.summary.selectedTurnId).toBe('source')
    expect(saved.turns[1]?.text).toBe('Original answer')
    const regenerated = await chat.run(
      { ...request, model: null, conversationId: 'stored', regenerate: true },
      () => undefined,
    )
    expect((await required(chat.history).get('stored')).turns[3]).toMatchObject({
      id: regenerated.turnId,
      questionId: 'source',
      supersedesId: 'source',
    })
  })

  it('translates real source text and links a separate question beneath it', async () => {
    const chat = required(createFakePorts({ conversations: [stored([turn('source')])] }).chat)
    const accepted = await chat.run(
      { ...request, conversationId: 'stored', translateTurnId: 'source', targetLanguage: ' French ', input: 'Ignored' },
      () => undefined,
    )
    const saved = await required(chat.history).get('stored')
    expect(saved.turns[1]).toMatchObject({
      id: accepted.turnId,
      parentId: 'source',
      questionId: accepted.turnId,
      supersedesId: null,
      kind: 'translation',
      input:
        'Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\nOriginal answer',
    })
    const retried = await chat.run(
      { ...request, conversationId: 'stored', retryTurnId: accepted.turnId },
      () => undefined,
    )
    expect((await required(chat.history).get('stored')).turns[2]).toMatchObject({
      id: retried.turnId,
      kind: 'translation',
      questionId: accepted.turnId,
    })
  })

  it('stores request images as data URLs, reuses them on retry and never on translation', async () => {
    const chat = required(createFakePorts().chat)
    const images = [{ mime: 'image/png', data: 'AAAA' }]
    const first = await chat.run({ ...request, images }, () => undefined)
    const conversationId = required(first.conversationId)
    const shown = [{ mime: 'image/png', url: 'data:image/png;base64,AAAA' }]
    expect((await required(chat.history).get(conversationId)).turns[0]?.images).toEqual(shown)
    const retried = await chat.run({ ...request, conversationId, retryTurnId: first.turnId }, () => undefined)
    const translated = await chat.run(
      { ...request, conversationId, translateTurnId: first.turnId, targetLanguage: 'French' },
      () => undefined,
    )
    const turns = (await required(chat.history).get(conversationId)).turns
    expect(turns.find((item) => item.id === retried.turnId)?.images).toEqual(shown)
    expect(turns.find((item) => item.id === translated.turnId)?.images).toEqual([])
    const conflict = await chat
      .run({ ...request, conversationId, retryTurnId: first.turnId, images }, () => undefined)
      .then(
        () => null,
        (error: unknown) => (isKitClientError(error) ? error.details : null),
      )
    expect(conflict).toMatchObject({ field: 'images', reason: 'action_conflict' })
  })

  it('selects within one conversation and normal continuation uses that parent', async () => {
    const chat = required(
      createFakePorts({
        conversations: [stored([turn('source'), turn('candidate', { questionId: 'source', supersedesId: 'source' })])],
      }).chat,
    )
    expect((await required(chat.history).selectTurn('stored', 'candidate')).summary.id).toBe('stored')
    await chat.run({ ...request, conversationId: 'stored', input: 'Next' }, () => undefined)
    expect((await required(chat.history).get('stored')).turns[2]?.parentId).toBe('candidate')
    const other = await chat.run(request, () => undefined)
    expect(await code(required(chat.history).selectTurn('stored', required(other.turnId)))).toBe('not_found')
    expect((await required(chat.history).list()).length).toBe(2)
  })

  it('CAS edits only visible text, preserves generated usage and invalidates context', async () => {
    const seed = stored([turn('source')])
    const history = required(createFakePorts({ conversations: [seed] }).chat?.history)
    const saved = await history.editTurn('stored', 'source', '**Edited**', 'Original answer')
    expect(saved.turns[0]).toMatchObject({
      input: 'Question',
      text: '**Edited**',
      ...model,
      usage: seed.turns[0]?.usage,
      contextUsage: null,
      reasoningText: null,
    })
    expect(await code(history.editTurn('stored', 'source', 'lost', 'Original answer'))).toBe('conflict')
    expect(await code(history.editTurn('stored', 'source', ' ', '**Edited**'))).toBe('bad_request')
    expect(await code(history.editTurn('stored', 'source', 'x'.repeat(24_001), '**Edited**'))).toBe('bad_request')
    required(saved.turns[0]).text = 'External mutation'
    required(required(saved.turns[0]).usage).inputTokens = 0
    expect((await history.get('stored')).turns[0]?.text).toBe('**Edited**')
    expect((await history.get('stored')).turns[0]?.usage?.inputTokens).toBe(100)
    expect(seed.turns[0]?.text).toBe('Original answer')
  })

  it('deletes a selected or dependent reply by reparenting onto its successor', async () => {
    const history = required(
      createFakePorts({
        conversations: [
          stored(
            [
              turn('source'),
              turn('candidate', { questionId: 'source', supersedesId: 'source' }),
              turn('child', { parentId: 'candidate' }),
            ],
            'child',
          ),
        ],
      }).chat?.history,
    )
    expect(await code(history.editTurn('stored', 'source', 'changed', 'Original answer'))).toBe('conflict')
    const afterCandidate = await history.removeTurn('stored', 'candidate')
    expect(afterCandidate.turns.map(({ id }) => id)).toEqual(['source', 'child'])
    expect(afterCandidate.turns.find(({ id }) => id === 'child')?.parentId).toBe('source')
    expect(afterCandidate.summary.selectedTurnId).toBe('child')
    const afterChild = await history.removeTurn('stored', 'child')
    expect(afterChild.summary.selectedTurnId).toBe('source')
    expect((await history.removeTurn('stored', 'source')).summary.selectedTurnId).toBeNull()
  })

  it('forks only chosen ancestry with fresh IDs, remapped selection and independent storage', async () => {
    const seed = stored(
      [
        turn('source'),
        turn('candidate', { questionId: 'source', supersedesId: 'source' }),
        turn('child', { parentId: 'candidate' }),
      ],
      'child',
    )
    const history = required(createFakePorts({ conversations: [seed] }).chat?.history)
    const fork = await history.fork('stored', 'child')
    expect(fork.summary.id).not.toBe('stored')
    expect(fork.summary.scope).toBe('scope')
    expect(fork.turns).toHaveLength(2)
    expect(fork.turns[0]?.supersedesId).toBeNull()
    expect(fork.turns[1]?.parentId).toBe(fork.turns[0]?.id)
    expect(fork.summary.selectedTurnId).toBe(fork.turns[1]?.id)
    for (const copied of fork.turns) {
      expect(seed.turns.some((source) => source.id === copied.id || source.questionId === copied.questionId)).toBe(
        false,
      )
    }
    await history.editTurn(fork.summary.id, required(fork.turns[1]).id, 'Fork only', 'Original answer')
    expect((await history.get('stored')).turns[2]?.text).toBe('Original answer')
  })

  it('rejects active conversation mutations and fork', async () => {
    const history = required(
      createFakePorts({
        conversations: [stored([turn('source'), turn('running', { status: 'running' })])],
      }).chat?.history,
    )
    expect(await code(history.editTurn('stored', 'source', 'changed', 'Original answer'))).toBe('conflict')
    expect(await code(history.removeTurn('stored', 'source'))).toBe('conflict')
    expect(await code(history.selectTurn('stored', 'source'))).toBe('conflict')
    expect(await code(history.fork('stored', 'source'))).toBe('conflict')
  })

  it('rejects conflicting actions, missing languages, writing graph fields and removed source models', async () => {
    const chat = required(createFakePorts({ conversations: [stored([turn('source')])] }).chat)
    const invalid: Partial<RunRequest>[] = [
      { retryTurnId: 'source', translateTurnId: 'source', targetLanguage: 'French' },
      { retryTurnId: 'source', regenerate: true },
      { translateTurnId: 'source', regenerate: true },
      { retryTurnId: 'source', webSearch: true },
      { translateTurnId: 'source' },
      { targetLanguage: 'French' },
      { replyToId: 'source', retryTurnId: 'source' },
      { purpose: 'writing', replyToId: 'source' },
      { purpose: 'writing', retryTurnId: 'source' },
      { purpose: 'writing', translateTurnId: 'source', targetLanguage: 'French' },
    ]
    for (const fields of invalid)
      expect(await code(chat.run({ ...request, conversationId: 'stored', ...fields }, () => undefined))).toBe(
        'bad_request',
      )
    const removed = required(
      createFakePorts({
        conversations: [stored([turn('source')])],
        providers: { modelOptions: () => Promise.resolve([]) },
      }).chat,
    )
    expect(
      await code(
        removed.run({ ...request, model: null, conversationId: 'stored', retryTurnId: 'source' }, () => undefined),
      ),
    ).toBe('needs_config')
    expect(
      await code(chat.run({ ...request, conversationId: 'stored', retryTurnId: 'missing' }, () => undefined)),
    ).toBe('not_found')
    expect(await code(chat.run({ ...request, conversationId: 'stored', replyToId: 'missing' }, () => undefined))).toBe(
      'not_found',
    )
    expect(await code(chat.run({ ...request, conversationId: 'missing' }, () => undefined))).toBe('not_found')
    expect((await required(chat.history).get('stored')).turns).toHaveLength(1)
    expect(await code(chat.run({ ...request, retryTurnId: 'source' }, () => undefined))).toBe('bad_request')
  })

  it('skills.get rejects with not_found and its details', async () => {
    const error: unknown = await createFakePorts()
      .skills?.get('s1')
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && [error.code, error.details.entity]).toEqual(['not_found', 'skill'])
  })
})
