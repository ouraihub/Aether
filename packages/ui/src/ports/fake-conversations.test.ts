import { describe, expect, it } from 'vitest'
import { createFakePorts } from './index'
import { code, request, required, stored, turn } from './test-support/fake-fixture'

describe('fake conversations port', () => {
  it('lists scoped storage, renames and removes real conversations', async () => {
    const conversations = required(
      createFakePorts({ initialConversations: [stored([turn('source')])] }).conversations,
    )
    expect(await conversations.list('other')).toEqual([])
    expect((await conversations.list('scope'))[0]?.id).toBe('stored')
    expect((await conversations.rename('stored', 'Renamed')).title).toBe('Renamed')
    const listed = await conversations.list()
    required(listed[0]).title = 'External mutation'
    expect((await conversations.get('stored')).summary.title).toBe('Renamed')
    await conversations.remove('stored')
    expect(await conversations.list()).toEqual([])
    expect(await code(conversations.get('stored'))).toBe('not_found')
  })

  it('rejects cyclic and missing fork ancestors without creating partial conversations', async () => {
    for (const parentId of ['source', 'missing']) {
      const conversations = required(
        createFakePorts({ initialConversations: [stored([turn('source', { parentId })])] })
          .conversations,
      )
      expect(await code(conversations.fork('stored', 'source'))).toBe(
        parentId === 'source' ? 'conflict' : 'not_found',
      )
      expect(await conversations.list()).toHaveLength(1)
    }
  })

  it('selects within one conversation and normal continuation uses that parent', async () => {
    const ports = createFakePorts({
      initialConversations: [
        stored([
          turn('source'),
          turn('candidate', { questionId: 'source', supersedesId: 'source' }),
        ]),
      ],
    })
    const chat = required(ports.chat)
    expect((await required(ports.conversations).selectTurn('stored', 'candidate')).summary.id).toBe(
      'stored',
    )
    await chat.start({ ...request, input: 'Next', conversationId: 'stored' }, () => undefined)
    expect((await required(ports.conversations).get('stored')).turns[2]?.parentId).toBe('candidate')
    const other = await chat.start(request, () => undefined)
    expect(
      await code(required(ports.conversations).selectTurn('stored', required(other.turnId))),
    ).toBe('not_found')
    expect((await required(ports.conversations).list()).length).toBe(2)
  })

  it('CAS edits only visible text, preserves generated usage and invalidates context', async () => {
    const seed = stored([turn('source')])
    const conversations = required(createFakePorts({ initialConversations: [seed] }).conversations)
    const saved = await conversations.editTurn('stored', 'source', '**Edited**', 'Original answer')
    expect(saved.turns[0]).toMatchObject({
      input: 'Question',
      text: '**Edited**',
      endpointId: 'endpoint',
      modelId: 'model',
      usage: seed.turns[0]?.usage,
      contextUsage: null,
      reasoningText: null,
    })
    expect(await code(conversations.editTurn('stored', 'source', 'lost', 'Original answer'))).toBe(
      'conflict',
    )
    expect(await code(conversations.editTurn('stored', 'source', ' ', '**Edited**'))).toBe(
      'bad_request',
    )
    expect(
      await code(conversations.editTurn('stored', 'source', 'x'.repeat(24_001), '**Edited**')),
    ).toBe('bad_request')
    required(saved.turns[0]).text = 'External mutation'
    required(required(saved.turns[0]).usage).inputTokens = 0
    expect((await conversations.get('stored')).turns[0]?.text).toBe('**Edited**')
    expect((await conversations.get('stored')).turns[0]?.usage?.inputTokens).toBe(100)
    expect(seed.turns[0]?.text).toBe('Original answer')
  })

  it('deletes a selected or dependent reply by reparenting onto its successor', async () => {
    const conversations = required(
      createFakePorts({
        initialConversations: [
          stored(
            [
              turn('source'),
              turn('candidate', { questionId: 'source', supersedesId: 'source' }),
              turn('child', { parentId: 'candidate' }),
            ],
            'child',
          ),
        ],
      }).conversations,
    )
    expect(
      await code(conversations.editTurn('stored', 'source', 'changed', 'Original answer')),
    ).toBe('conflict')
    const afterCandidate = await conversations.removeTurn('stored', 'candidate')
    expect(afterCandidate.turns.map(({ id }) => id)).toEqual(['source', 'child'])
    expect(afterCandidate.turns.find(({ id }) => id === 'child')?.parentId).toBe('source')
    expect(afterCandidate.summary.selectedTurnId).toBe('child')
    const afterChild = await conversations.removeTurn('stored', 'child')
    expect(afterChild.summary.selectedTurnId).toBe('source')
    expect((await conversations.removeTurn('stored', 'source')).summary.selectedTurnId).toBeNull()
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
    const conversations = required(createFakePorts({ initialConversations: [seed] }).conversations)
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
          (source) => source.id === copied.id || source.questionId === copied.questionId,
        ),
      ).toBe(false)
    }
    await conversations.editTurn(
      fork.summary.id,
      required(fork.turns[1]).id,
      'Fork only',
      'Original answer',
    )
    expect((await conversations.get('stored')).turns[2]?.text).toBe('Original answer')
  })

  it('rejects active conversation mutations and fork', async () => {
    const conversations = required(
      createFakePorts({
        initialConversations: [stored([turn('source'), turn('running', { status: 'running' })])],
      }).conversations,
    )
    expect(
      await code(conversations.editTurn('stored', 'source', 'changed', 'Original answer')),
    ).toBe('conflict')
    expect(await code(conversations.removeTurn('stored', 'source'))).toBe('conflict')
    expect(await code(conversations.selectTurn('stored', 'source'))).toBe('conflict')
    expect(await code(conversations.fork('stored', 'source'))).toBe('conflict')
  })
})
