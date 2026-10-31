import { describe, expect, it } from 'vitest'
import { createFakePorts, isKitClientError } from './index'
import type { Conversation, RunEvent, RunRequest, Turn } from './index'

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected fixture is missing')
  return value
}

const model = { endpointId: 'endpoint', modelId: 'model' }
const request: RunRequest = {
  purpose: 'chat',
  skills: [],
  input: 'Question',
  regenerate: false,
  language: 'en',
  model,
}
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
  usage: {
    inputTokens: 100,
    outputTokens: 60,
    cacheReadTokens: 20,
    cacheWriteTokens: null,
    reasoningTokens: 4,
  },
  contextUsage: {
    tokens: 160,
    contextWindow: 1000,
    source: 'reported',
    ...model,
  },
  ...overrides,
})
const stored = (
  turns: Turn[],
  selectedTurnId: string | null = turns[0]?.id ?? null,
): Conversation => ({
  summary: {
    id: 'stored',
    title: 'Stored',
    scope: 'scope',
    createdAt: 1,
    updatedAt: 2,
    selectedTurnId,
  },
  turns,
})

async function code(promise: Promise<unknown>) {
  const error: unknown = await promise.catch((caught: unknown) => caught)
  return isKitClientError(error) ? error.code : null
}

describe('createFakePorts conversation behavior', () => {
  it('lists scoped storage, renames and removes conversations', async () => {
    const conversations = required(
      createFakePorts({ initialConversations: [stored([turn('source')])] })
        .conversations,
    )
    expect(await conversations.list('other')).toEqual([])
    expect((await conversations.list('scope'))[0]?.id).toBe('stored')
    expect((await conversations.rename('stored', 'Renamed')).title).toBe(
      'Renamed',
    )
    const listed = await conversations.list()
    required(listed[0]).title = 'External mutation'
    expect((await conversations.get('stored')).summary.title).toBe('Renamed')
    await conversations.remove('stored')
    expect(await conversations.list()).toEqual([])
    expect(await code(conversations.get('stored'))).toBe('not_found')
  })

  it('rejects cyclic or missing fork ancestors', async () => {
    for (const parentId of ['source', 'missing']) {
      const conversations = required(
        createFakePorts({
          initialConversations: [stored([turn('source', { parentId })])],
        }).conversations,
      )
      expect(await code(conversations.fork('stored', 'source'))).toBe(
        parentId === 'source' ? 'conflict' : 'not_found',
      )
      expect(await conversations.list()).toHaveLength(1)
    }
  })

  it('supports independent conversations and end-only runs', async () => {
    const ports = createFakePorts({ omit: ['conversations'] })
    const chat = required(ports.chat)
    expect(ports.conversations).toBeUndefined()
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event))
    expect(accepted).toMatchObject({ conversationId: null, turnId: null })
    await Promise.resolve()
    expect(events).toEqual([{ kind: 'end', status: 'completed' }])
    expect(
      await code(
        chat.start(request, () => undefined, { signal: AbortSignal.abort() }),
      ),
    ).toBe('cancelled')
  })

  it('keeps writing runs temporary with available conversations', async () => {
    const ports = createFakePorts()
    const events: RunEvent[] = []
    const accepted = await required(ports.chat).start(
      { ...request, purpose: 'writing' },
      (event) => events.push(event),
    )
    expect(accepted).toMatchObject({ conversationId: null, turnId: null })
    await Promise.resolve()
    expect(events).toEqual([{ kind: 'end', status: 'completed' }])
    expect(await required(ports.conversations).list()).toEqual([])
    expect(
      await required(ports.chat).getSnapshot(accepted.runId),
    ).toMatchObject({ status: 'completed' })
  })

  it('preserves candidate question, parent and model on retry', async () => {
    const ports = createFakePorts({
      initialConversations: [
        stored(
          [turn('parent'), turn('source', { parentId: 'parent' })],
          'source',
        ),
      ],
    })
    const chat = required(ports.chat)
    const accepted = await chat.start(
      {
        ...request,
        model: null,
        conversationId: 'stored',
        retryTurnId: 'source',
        input: 'Ignored',
      },
      () => undefined,
    )
    const saved = await required(ports.conversations).get('stored')
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
    const regenerated = await chat.start(
      { ...request, model: null, conversationId: 'stored', regenerate: true },
      () => undefined,
    )
    expect(
      (await required(ports.conversations).get('stored')).turns[3],
    ).toMatchObject({
      id: regenerated.turnId,
      questionId: 'source',
      supersedesId: 'source',
    })
  })

  it('translates source text and links a question beneath it', async () =>
  {
    const ports = createFakePorts({
      initialConversations: [stored([turn('source')])],
    })
    const chat = required(ports.chat)
    const accepted = await chat.start(
      {
        ...request,
        conversationId: 'stored',
        translateTurnId: 'source',
        targetLanguage: ' French ',
        input: 'Ignored',
      },
      () => undefined,
    )
    const saved = await required(ports.conversations).get('stored')
    expect(saved.turns[1]).toMatchObject({
      id: accepted.turnId,
      parentId: 'source',
      questionId: accepted.turnId,
      supersedesId: null,
      kind: 'translation',
      input:
        'Translate the following reply into French. Return only the '
        + 'translation, preserving Markdown formatting.\n\nOriginal answer',
    })
    const retried = await chat.start(
      { ...request, conversationId: 'stored', retryTurnId: accepted.turnId },
      () => undefined,
    )
    expect(
      (await required(ports.conversations).get('stored')).turns[2],
    ).toMatchObject({
      id: retried.turnId,
      kind: 'translation',
      questionId: accepted.turnId,
    })
  })

  it('stores images, reuses retry images, omits translation images', async () =>
  {
    const ports = createFakePorts()
    const chat = required(ports.chat)
    const images = [{ mime: 'image/png', data: 'AAAA' }]
    const first = await chat.start({ ...request, images }, () => undefined)
    const conversationId = required(first.conversationId)
    const shown = [{ mime: 'image/png', url: 'data:image/png;base64,AAAA' }]
    expect(
      (await required(ports.conversations).get(conversationId)).turns[0]
        ?.images,
    ).toEqual(shown)
    const retried = await chat.start(
      { ...request, conversationId, retryTurnId: first.turnId },
      () => undefined,
    )
    await Promise.resolve()
    const translated = await chat.start(
      {
        ...request,
        conversationId,
        translateTurnId: first.turnId,
        targetLanguage: 'French',
      },
      () => undefined,
    )
    const turns = (await required(ports.conversations).get(conversationId))
      .turns
    expect(turns.find((item) => item.id === retried.turnId)?.images).toEqual(
      shown,
    )
    expect(turns.find((item) => item.id === translated.turnId)?.images).toEqual(
      [],
    )
    const conflict = await chat
      .start(
        { ...request, conversationId, retryTurnId: first.turnId, images },
        () => undefined,
      )
      .then(
        () => null,
        (error: unknown) => (isKitClientError(error) ? error.details : null),
      )
    expect(conflict).toMatchObject({
      field: 'images',
      reason: 'action_conflict',
    })
  })

  it('selects within one conversation and uses that parent', async () => {
    const ports = createFakePorts({
      initialConversations: [
        stored([
          turn('source'),
          turn('candidate', { questionId: 'source', supersedesId: 'source' }),
        ]),
      ],
    })
    const chat = required(ports.chat)
    expect(
      (await required(ports.conversations).selectTurn('stored', 'candidate'))
        .summary.id,
    ).toBe('stored')
    await chat.start(
      { ...request, conversationId: 'stored', input: 'Next' },
      () => undefined,
    )
    expect(
      (await required(ports.conversations).get('stored')).turns[2]?.parentId,
    ).toBe('candidate')
    const other = await chat.start(request, () => undefined)
    expect(
      await code(
        required(ports.conversations).selectTurn(
          'stored',
          required(other.turnId),
        ),
      ),
    ).toBe('not_found')
    expect((await required(ports.conversations).list()).length).toBe(2)
  })

  it('CAS edits text, preserves usage and invalidates context', async () =>
  {
    const seed = stored([turn('source')])
    const conversations = required(
      createFakePorts({ initialConversations: [seed] }).conversations,
    )
    const saved = await conversations.editTurn(
      'stored',
      'source',
      '**Edited**',
      'Original answer',
    )
    expect(saved.turns[0]).toMatchObject({
      input: 'Question',
      text: '**Edited**',
      ...model,
      usage: seed.turns[0]?.usage,
      contextUsage: null,
      reasoningText: null,
    })
    expect(
      await code(
        conversations.editTurn('stored', 'source', 'lost', 'Original answer'),
      ),
    ).toBe('conflict')
    expect(
      await code(conversations.editTurn('stored', 'source', ' ', '**Edited**')),
    ).toBe('bad_request')
    expect(
      await code(
        conversations.editTurn(
          'stored',
          'source',
          'x'.repeat(24_001),
          '**Edited**',
        ),
      ),
    ).toBe('bad_request')
    required(saved.turns[0]).text = 'External mutation'
    required(required(saved.turns[0]).usage).inputTokens = 0
    expect((await conversations.get('stored')).turns[0]?.text).toBe(
      '**Edited**',
    )
    expect(
      (await conversations.get('stored')).turns[0]?.usage?.inputTokens,
    ).toBe(100)
    expect(seed.turns[0]?.text).toBe('Original answer')
  })

  it('deletes a reply by reparenting onto its successor', async () => {
    const conversations = required(
      createFakePorts({
        initialConversations: [
          stored(
            [
              turn('source'),
              turn('candidate', {
                questionId: 'source',
                supersedesId: 'source',
              }),
              turn('child', { parentId: 'candidate' }),
            ],
            'child',
          ),
        ],
      }).conversations,
    )
    expect(
      await code(
        conversations.editTurn(
          'stored',
          'source',
          'changed',
          'Original answer',
        ),
      ),
    ).toBe('conflict')
    const afterCandidate = await conversations.removeTurn('stored', 'candidate')
    expect(afterCandidate.turns.map(({ id }) => id)).toEqual([
      'source',
      'child',
    ])
    expect(
      afterCandidate.turns.find(({ id }) => id === 'child')?.parentId,
    ).toBe('source')
    expect(afterCandidate.summary.selectedTurnId).toBe('child')
    const afterChild = await conversations.removeTurn('stored', 'child')
    expect(afterChild.summary.selectedTurnId).toBe('source')
    expect(
      (await conversations.removeTurn('stored', 'source')).summary
        .selectedTurnId,
    ).toBeNull()
  })

  it('forks ancestry with fresh IDs and independent storage', async () =>
  {
    const seed = stored(
      [
        turn('source'),
        turn('candidate', { questionId: 'source', supersedesId: 'source' }),
        turn('child', { parentId: 'candidate' }),
      ],
      'child',
    )
    const conversations = required(
      createFakePorts({ initialConversations: [seed] }).conversations,
    )
    const fork = await conversations.fork('stored', 'child')
    expect(fork.summary.id).not.toBe('stored')
    expect(fork.summary.scope).toBe('scope')
    expect(fork.turns).toHaveLength(2)
    expect(fork.turns[0]?.supersedesId).toBeNull()
    expect(fork.turns[1]?.parentId).toBe(fork.turns[0]?.id)
    expect(fork.summary.selectedTurnId).toBe(fork.turns[1]?.id)
    for (const copied of fork.turns) {
      expect(
        seed.turns.some(
          (source) =>
            source.id === copied.id || source.questionId === copied.questionId,
        ),
      ).toBe(false)
    }
    await conversations.editTurn(
      fork.summary.id,
      required(fork.turns[1]).id,
      'Fork only',
      'Original answer',
    )
    expect((await conversations.get('stored')).turns[2]?.text).toBe(
      'Original answer',
    )
  })

  it('rejects active conversation mutations and fork', async () => {
    const conversations = required(
      createFakePorts({
        initialConversations: [
          stored([turn('source'), turn('running', { status: 'running' })]),
        ],
      }).conversations,
    )
    expect(
      await code(
        conversations.editTurn(
          'stored',
          'source',
          'changed',
          'Original answer',
        ),
      ),
    ).toBe('conflict')
    expect(await code(conversations.removeTurn('stored', 'source'))).toBe(
      'conflict',
    )
    expect(await code(conversations.selectTurn('stored', 'source'))).toBe(
      'conflict',
    )
    expect(await code(conversations.fork('stored', 'source'))).toBe('conflict')
  })


})
