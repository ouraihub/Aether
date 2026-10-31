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

describe('createFakePorts', () => {
  it('declares every port by default', () => {
    expect(Object.keys(createFakePorts()).sort()).toEqual(
      [
        'chat',
        'conversations',
        'desktop',
        'logs',
        'mcpClient',
        'mcpServer',
        'providers',
        'skills',
      ].sort(),
    )
  })

  it('omits ports like a host without the feature', () => {
    const ports = createFakePorts({ omit: ['chat', 'mcpServer'] })
    expect(ports.chat).toBeUndefined()
    expect(ports.mcpServer).toBeUndefined()
    expect(ports.providers).toBeDefined()
    expect(ports.conversations).toBeDefined()
    expect(ports.desktop).toBeDefined()
  })

  it('overrides single methods and keeps the rest', async () => {
    const ports = createFakePorts({
      providers: {
        list: () =>
          Promise.resolve([
            {
              id: 'p',
              name: 'P',
              enabled: true,
              notes: '',
              presetId: null,
              endpoints: [],
            },
          ]),
      },
    })
    expect(await ports.providers?.list()).toEqual([
      {
        id: 'p',
        name: 'P',
        enabled: true,
        notes: '',
        presetId: null,
        endpoints: [],
      },
    ])
    expect(await ports.providers?.modelOptions()).toEqual([])
  })

  it('overrides conversation and desktop independently', async () => {
    let opened = 0
    const ports = createFakePorts({
      initialConversations: [stored([turn('source')])],
      conversations: { list: () => Promise.resolve([]) },
      desktop: {
        openLogsDirectory: async () => {
          opened++
        },
      },
      omit: ['chat'],
    })
    expect(ports.chat).toBeUndefined()
    const conversations = required(ports.conversations)
    expect(await conversations.list()).toEqual([])
    expect((await conversations.get('stored')).turns[0]?.id).toBe('source')
    await required(ports.desktop).openLogsDirectory()
    expect(opened).toBe(1)
    expect(await required(ports.logs).tail(1)).toEqual([])
  })

  it('requests cancellation and persists one terminal', async () => {
    const ports = createFakePorts()
    const chat = required(ports.chat)
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event))
    const before = chat.getSnapshot(accepted.runId)
    const cancellation = chat.cancel(accepted.runId)
    const requested = chat.getSnapshot(accepted.runId)
    void chat.cancel(accepted.runId)
    expect(events).toEqual([])
    expect(await before).toEqual({
      ...accepted,
      status: 'running',
      cancelRequested: false,
    })
    expect(await requested).toEqual({
      ...accepted,
      status: 'running',
      cancelRequested: true,
    })
    await cancellation
    expect(events).toEqual([{ kind: 'end', status: 'cancelled' }])
    expect(await chat.getSnapshot(accepted.runId)).toEqual({
      ...accepted,
      status: 'cancelled',
      cancelRequested: true,
    })
    expect(
      (
        await required(ports.conversations).get(
          required(accepted.conversationId),
        )
      ).turns[0],
    ).toMatchObject({
      status: 'cancelled',
      text: '',
    })
    await chat.cancel(accepted.runId)
    await chat.cancel('unknown')
    expect(events.filter((event) => event.kind === 'end')).toHaveLength(1)
    expect(await code(chat.getSnapshot('unknown'))).toBe('not_found')
    expect(await code(chat.approve('unknown', 'call', true))).toBe('not_found')
    expect(await code(chat.approve(accepted.runId, 'call', true))).toBe(
      'conflict',
    )
  })

  it('reports completed snapshots without mutable storage', async () => {
    const chat = required(createFakePorts().chat)
    const events: RunEvent[] = []
    const accepted = await chat.start(request, (event) => events.push(event))
    await Promise.resolve()
    const snapshot = await chat.getSnapshot(accepted.runId)
    expect(snapshot).toEqual({
      ...accepted,
      status: 'completed',
      cancelRequested: false,
    })
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

  it('routes accepted abort through cancellation', async () => {
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
    const completed = await chat.start(request, () => undefined, {
      signal: finished.signal,
    })
    await Promise.resolve()
    finished.abort()
    expect(await chat.getSnapshot(completed.runId)).toMatchObject({
      status: 'completed',
      cancelRequested: false,
    })
  })

  it('retains terminal snapshots and uses unique no-store IDs', async () => {
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
    expect(await code(chat.approve(required(ids[0]), 'call', false))).toBe(
      'not_found',
    )
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
    expect(await running).toEqual({
      ...active,
      status: 'running',
      cancelRequested: false,
    })
    expect(await code(chat.getSnapshot(required(ids[1])))).toBe('not_found')
    expect(await chat.getSnapshot(active.runId)).toMatchObject({
      status: 'completed',
    })
  })

  it('accepts stable IDs and persists before emitting one end', async () => {
    const ports = createFakePorts()
    const chat = required(ports.chat)
    const events: RunEvent[] = []
    let persisted: Promise<Conversation> | undefined
    const run = chat.start(request, (event) => {
      events.push(event)
      if (event.kind === 'end')
        persisted = required(ports.conversations).get('fake-conversation-1')
    })
    expect(events).toEqual([])
    const accepted = await run
    expect(accepted).toMatchObject({
      conversationId: 'fake-conversation-1',
      turnId: 'fake-turn-1',
    })
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
    expect(
      (await required(ports.conversations).list())[0]?.selectedTurnId,
    ).toBe(accepted.turnId)
  })
  it('skills.get rejects with not_found and its details', async () => {
    const error: unknown = await createFakePorts()
      .skills?.get('s1')
      .catch((caught: unknown) => caught)
    expect(
      isKitClientError(error) && [error.code, error.details.entity],
    ).toEqual(['not_found', 'skill'])
  })
})
