import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { isKitClientError } from '../ports'
import type { Conversation, RunRequest } from '../ports'
import { createTauriPorts } from './index'

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected fixture is missing')
  return value
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {
    constructor(readonly onmessage: unknown) {}
  },
}))

const invokeMock = vi.mocked(invoke)
const REQUEST = { purpose: 'chat', skills: [], input: 'hi', regenerate: false, language: 'en' }

beforeEach(() => invokeMock.mockReset())

describe('createTauriPorts', () => {
  it('creates only the declared ports', () => {
    expect(Object.keys(createTauriPorts())).toEqual(['logs'])
    expect(Object.keys(createTauriPorts({ providers: true, chat: true })).sort()).toEqual(['chat', 'logs', 'providers'])
  })

  it('turns a KitError rejection into KitClientError', async () => {
    invokeMock.mockRejectedValueOnce({ code: 'conflict', message: 'exists', details: { reason: 'exists' } })
    const error: unknown = await createTauriPorts({ skills: true })
      .skills?.list()
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && [error.code, error.message, error.details.reason]).toEqual([
      'conflict',
      'exists',
      'exists',
    ])
    expect(invokeMock).toHaveBeenCalledWith('plugin:aether|skills_list', {})
  })
  it('maps history turn mutations to native commands', async () => {
    const conversation: Conversation = {
      summary: { id: 'c1', title: 'Conversation', scope: null, createdAt: 0, updatedAt: 0, selectedTurnId: null },
      turns: [],
    }
    invokeMock.mockResolvedValue(conversation)
    const history = createTauriPorts({ chat: true, chatHistory: true }).chat?.history
    await history?.editTurn('c1', 't1', 'new', 'old')
    await history?.removeTurn('c1', 't1')
    await history?.fork('c1', 't1')
    await history?.selectTurn('c1', 't1')
    expect(invokeMock.mock.calls.slice(0, 4)).toEqual([
      ['plugin:aether|chat_edit_turn', { conversationId: 'c1', turnId: 't1', text: 'new', expectedText: 'old' }],
      ['plugin:aether|chat_remove_turn', { conversationId: 'c1', turnId: 't1' }],
      ['plugin:aether|chat_fork', { conversationId: 'c1', turnId: 't1' }],
      ['plugin:aether|chat_select_turn', { conversationId: 'c1', turnId: 't1' }],
    ])
  })

  it('propagates every history mutation KitError unchanged', async () => {
    const history = required(createTauriPorts({ chat: true, chatHistory: true }).chat?.history)
    const actions = [
      () => history.editTurn('c1', 't1', 'new', 'old'),
      () => history.removeTurn('c1', 't1'),
      () => history.fork('c1', 't1'),
      () => history.selectTurn('c1', 't1'),
    ]
    for (const action of actions) {
      invokeMock.mockRejectedValueOnce({ code: 'conflict', message: 'changed', details: { reason: 'text_changed' } })
      const error: unknown = await action().catch((caught: unknown) => caught)
      expect(isKitClientError(error) && [error.code, error.message, error.details.reason]).toEqual([
        'conflict',
        'changed',
        'text_changed',
      ])
    }
  })

  it('passes reply metadata and stable accepted IDs without transforming them', async () => {
    const accepted = { runId: 'run', conversationId: 'conversation', turnId: 'candidate' }
    invokeMock.mockResolvedValueOnce(accepted)
    const request: RunRequest = {
      ...REQUEST,
      conversationId: 'conversation',
      retryTurnId: 'original',
      model: { endpointId: 'endpoint', modelId: 'model' },
    }
    expect(await required(createTauriPorts({ chat: true }).chat).run(request, () => undefined)).toEqual(accepted)
    expect(invokeMock.mock.calls[0]?.[0]).toBe('plugin:aether|chat_run')
    expect(invokeMock.mock.calls[0]?.[1]).toMatchObject({ request })
  })

  it('maps a plain Tauri error (e.g. ACL rejection) to internal', async () => {
    invokeMock.mockRejectedValueOnce('kit.logs_tail not allowed. Command not found')
    const error: unknown = await createTauriPorts()
      .logs.tail(10)
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && [error.code, error.message]).toEqual([
      'internal',
      'kit.logs_tail not allowed. Command not found',
    ])
  })

  it('aborting a probe calls providers_cancel with the same job id', async () => {
    const controller = new AbortController()
    invokeMock.mockImplementation((command) => {
      if (command === 'plugin:aether|providers_probe') {
        controller.abort()
        return Promise.reject({ code: 'cancelled', message: 'cancelled' })
      }
      return Promise.resolve({ ok: true })
    })
    const error: unknown = await createTauriPorts({ providers: true })
      .providers?.probe({ kind: 'saved', endpointId: 'e1' }, { signal: controller.signal })
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && error.code).toBe('cancelled')
    const probeArgs = invokeMock.mock.calls[0]?.[1] as { jobId: string }
    expect(invokeMock.mock.calls[1]).toEqual(['plugin:aether|providers_cancel', { jobId: probeArgs.jobId }])
  })

  it('aborting an accepted run calls chat_cancel once', async () => {
    invokeMock.mockResolvedValue({ runId: 'r1', conversationId: null, turnId: null })
    const controller = new AbortController()
    const chat = createTauriPorts({ chat: true }).chat
    expect(await chat?.run(REQUEST, () => undefined, { signal: controller.signal })).toEqual({
      runId: 'r1',
      conversationId: null,
      turnId: null,
    })
    controller.abort()
    controller.abort()
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      'plugin:aether|chat_run',
      'plugin:aether|chat_cancel',
    ])
    expect(invokeMock.mock.calls[1]?.[1]).toEqual({ runId: 'r1' })
  })

  it('an already aborted run never reaches the backend', async () => {
    const error: unknown = await createTauriPorts({ chat: true })
      .chat?.run(REQUEST, () => undefined, { signal: AbortSignal.abort() })
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && error.code).toBe('cancelled')
    expect(invokeMock).not.toHaveBeenCalled()
  })
})
