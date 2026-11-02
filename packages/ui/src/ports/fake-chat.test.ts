import { describe, expect, it } from 'vitest'
import { createFakePorts, isKitClientError } from './index'
import type { Conversation, RunEvent, RunRequest } from './index'
import { code, model, request, required, stored, turn } from './test-support/fake-fixture'

describe('fake chat port', () => {
  it('requests cancellation idempotently and persists one cancelled terminal', async () => {
    const ports = createFakePorts()
    const chat = required(ports.chat)
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event))
    const before = chat.getSnapshot(accepted.runId)
    const cancellation = chat.cancel(accepted.runId)
    const requested = chat.getSnapshot(accepted.runId)
    void chat.cancel(accepted.runId)
    expect(events).toEqual([])
    expect(await before).toEqual({ ...accepted, status: 'running', cancelRequested: false })
    expect(await requested).toEqual({ ...accepted, status: 'running', cancelRequested: true })
    await cancellation
    expect(events).toEqual([{ kind: 'end', status: 'cancelled' }])
    expect(await chat.getSnapshot(accepted.runId)).toEqual({
      ...accepted,
      status: 'cancelled',
      cancelRequested: true,
    })
    expect(
      (await required(ports.conversations).get(required(accepted.conversationId))).turns[0],
    ).toMatchObject({
      status: 'cancelled',
      text: '',
    })
    await chat.cancel(accepted.runId)
    await chat.cancel('unknown')
    expect(events.filter((event) => event.kind === 'end')).toHaveLength(1)
    expect(await code(chat.getSnapshot('unknown'))).toBe('not_found')
    expect(await code(chat.approve('unknown', 'call', true))).toBe('not_found')
    expect(await code(chat.approve(accepted.runId, 'call', true))).toBe('conflict')
  })

  it('reports completed snapshots without mutable storage or late cancellation', async () => {
    const chat = required(createFakePorts().chat)
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event))
    await Promise.resolve()
    const snapshot = await chat.getSnapshot(accepted.runId)
    expect(snapshot).toEqual({ ...accepted, status: 'completed', cancelRequested: false })
    snapshot.status = 'failed'
    await chat.cancel(accepted.runId)
    expect(await chat.getSnapshot(accepted.runId)).toEqual({
      ...accepted,
      status: 'completed',
      cancelRequested: false,
    })
    expect(events.filter((event) => event.kind === 'end')).toEqual([
      { kind: 'end', status: 'completed' },
    ])
  })

  it('routes an accepted abort through cancellation and removes the listener at completion', async () => {
    const chat = required(createFakePorts().chat)
    const controller = new AbortController()
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event), {
      signal: controller.signal,
    })
    controller.abort()
    await Promise.resolve()
    expect(events).toEqual([{ kind: 'end', status: 'cancelled' }])
    expect(await chat.getSnapshot(accepted.runId)).toMatchObject({
      status: 'cancelled',
      cancelRequested: true,
    })

    const finished = new AbortController()
    const completed = await chat.start(request, () => undefined, { signal: finished.signal })
    await Promise.resolve()
    finished.abort()
    expect(await chat.getSnapshot(completed.runId)).toMatchObject({
      status: 'completed',
      cancelRequested: false,
    })
  })

  it('retains only the last 128 terminal snapshots and uses unique no-store run IDs', async () => {
    const chat = required(createFakePorts({ omit: ['conversations'] }).chat)
    const ids: string[] = []
    for (let index = 0; index < 129; index++) {
      const accepted = await chat.start(request, () => undefined)
      ids.push(accepted.runId)
      expect(accepted).toMatchObject({ conversationId: null, turnId: null })
    }
    await Promise.resolve()
    expect(new Set(ids).size).toBe(129)
    expect(await code(chat.getSnapshot(required(ids[0])))).toBe('not_found')
    expect(await code(chat.approve(required(ids[0]), 'call', false))).toBe('not_found')
    await chat.cancel(required(ids[0]))
    for (const id of ids.slice(1))
      expect(await chat.getSnapshot(id)).toMatchObject({
        runId: id,
        status: 'completed',
        cancelRequested: false,
      })
    const active = await chat.start(request, () => undefined)
    const retained = chat.getSnapshot(required(ids[1]))
    const running = chat.getSnapshot(active.runId)
    expect(await retained).toMatchObject({ status: 'completed' })
    expect(await running).toEqual({ ...active, status: 'running', cancelRequested: false })
    expect(await code(chat.getSnapshot(required(ids[1])))).toBe('not_found')
    expect(await chat.getSnapshot(active.runId)).toMatchObject({ status: 'completed' })
  })

  it('accepts stable stored IDs and persists before emitting the single end', async () => {
    const ports = createFakePorts()
    const chat = required(ports.chat)
    const events: RunEvent[] = []
    let persisted: Promise<Conversation> | undefined
    const run = chat.start(request, (event) => {
      events.push(event)
      if (event.kind === 'end') persisted = required(ports.conversations).get('fake-conversation-1')
    })
    expect(events).toEqual([])
    const accepted = await run
    expect(accepted).toMatchObject({ conversationId: 'fake-conversation-1', turnId: 'fake-turn-1' })
    await Promise.resolve()
    expect((await persisted)?.turns[0]).toMatchObject({
      id: accepted.turnId,
      status: 'completed',
      text: 'Fake reply: Question',
    })
    expect(events).toEqual([
      { kind: 'text_delta', text: 'Fake reply: Question' },
      { kind: 'end', status: 'completed' },
    ])
    expect((await required(ports.conversations).list())[0]?.selectedTurnId).toBe(accepted.turnId)
  })

  it('supports independent conversations and end-only runs without storage', async () => {
    const ports = createFakePorts({ omit: ['conversations'] })
    const chat = required(ports.chat)
    expect(ports.conversations).toBeUndefined()
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event))
    expect(accepted).toMatchObject({ conversationId: null, turnId: null })
    await Promise.resolve()
    expect(events).toEqual([{ kind: 'end', status: 'completed' }])
    expect(await code(chat.start(request, () => undefined, { signal: AbortSignal.abort() }))).toBe(
      'cancelled',
    )
  })

  it('keeps writing runs temporary even when conversations are available', async () => {
    const ports = createFakePorts()
    const events: RunEvent[] = []
    const accepted = await required(ports.chat).start({ ...request, purpose: 'writing' }, (event) =>
      events.push(event),
    )
    expect(accepted).toMatchObject({ conversationId: null, turnId: null })
    await Promise.resolve()
    expect(events).toEqual([{ kind: 'end', status: 'completed' }])
    expect(await required(ports.conversations).list()).toEqual([])
    expect(await required(ports.chat).getSnapshot(accepted.runId)).toMatchObject({
      status: 'completed',
    })
  })

  it('preserves candidate question, parent and source model without selecting retry', async () => {
    const ports = createFakePorts({
      initialConversations: [
        stored([turn('parent'), turn('source', { parentId: 'parent' })], 'source'),
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
    expect((await required(ports.conversations).get('stored')).turns[3]).toMatchObject({
      id: regenerated.turnId,
      questionId: 'source',
      supersedesId: 'source',
    })
  })

  it('translates real source text and links a separate question beneath it', async () => {
    const ports = createFakePorts({ initialConversations: [stored([turn('source')])] })
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
        'Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\nOriginal answer',
    })
    const retried = await chat.start(
      { ...request, conversationId: 'stored', retryTurnId: accepted.turnId },
      () => undefined,
    )
    expect((await required(ports.conversations).get('stored')).turns[2]).toMatchObject({
      id: retried.turnId,
      kind: 'translation',
      questionId: accepted.turnId,
    })
  })

  it('stores request images as data URLs, reuses them on retry and never on translation', async () => {
    const ports = createFakePorts()
    const chat = required(ports.chat)
    const images = [{ mime: 'image/png', data: 'AAAA' }]
    const first = await chat.start({ ...request, images }, () => undefined)
    const conversationId = required(first.conversationId)
    const shown = [{ mime: 'image/png', url: 'data:image/png;base64,AAAA' }]
    expect((await required(ports.conversations).get(conversationId)).turns[0]?.images).toEqual(
      shown,
    )
    const retried = await chat.start(
      { ...request, conversationId, retryTurnId: first.turnId },
      () => undefined,
    )
    await Promise.resolve()
    const translated = await chat.start(
      { ...request, conversationId, translateTurnId: first.turnId, targetLanguage: 'French' },
      () => undefined,
    )
    const turns = (await required(ports.conversations).get(conversationId)).turns
    expect(turns.find((item) => item.id === retried.turnId)?.images).toEqual(shown)
    expect(turns.find((item) => item.id === translated.turnId)?.images).toEqual([])
    const conflict = await chat
      .start({ ...request, conversationId, retryTurnId: first.turnId, images }, () => undefined)
      .then(
        () => null,
        (error: unknown) => (isKitClientError(error) ? error.details : null),
      )
    expect(conflict).toMatchObject({ field: 'images', reason: 'action_conflict' })
  })

  it('rejects conflicting actions, missing languages, writing graph fields and removed source models', async () => {
    const ports = createFakePorts({ initialConversations: [stored([turn('source')])] })
    const chat = required(ports.chat)
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
      expect(
        await code(
          chat.start({ ...request, conversationId: 'stored', ...fields }, () => undefined),
        ),
      ).toBe('bad_request')
    const removed = required(
      createFakePorts({
        initialConversations: [stored([turn('source')])],
        providers: { modelOptions: () => Promise.resolve([]) },
      }).chat,
    )
    expect(
      await code(
        removed.start(
          { ...request, model: null, conversationId: 'stored', retryTurnId: 'source' },
          () => undefined,
        ),
      ),
    ).toBe('needs_config')
    expect(
      await code(
        chat.start(
          { ...request, conversationId: 'stored', retryTurnId: 'missing' },
          () => undefined,
        ),
      ),
    ).toBe('not_found')
    expect(
      await code(chat.start({ ...request, conversationId: 'stored', replyToId: 'missing' }, () => undefined)),
    ).toBe('not_found')
    expect(await code(chat.start({ ...request, conversationId: 'missing' }, () => undefined))).toBe(
      'not_found',
    )
    expect((await required(ports.conversations).get('stored')).turns).toHaveLength(1)
    expect(await code(chat.start({ ...request, retryTurnId: 'source' }, () => undefined))).toBe(
      'bad_request',
    )
  })
})
