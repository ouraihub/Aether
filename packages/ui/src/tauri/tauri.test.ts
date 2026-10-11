import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { isKitClientError } from '../ports'
import { createTauriPorts } from './index'

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
    expect(invokeMock).toHaveBeenCalledWith('plugin:kit|skills_list', {})
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
      if (command === 'plugin:kit|providers_probe') {
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
    expect(invokeMock.mock.calls[1]).toEqual(['plugin:kit|providers_cancel', { jobId: probeArgs.jobId }])
  })

  it('aborting an accepted run calls chat_cancel once', async () => {
    invokeMock.mockResolvedValue({ runId: 'r1', conversationId: null })
    const controller = new AbortController()
    const chat = createTauriPorts({ chat: true }).chat
    expect(await chat?.run(REQUEST, () => undefined, { signal: controller.signal })).toEqual({
      runId: 'r1',
      conversationId: null,
    })
    controller.abort()
    controller.abort()
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(['plugin:kit|chat_run', 'plugin:kit|chat_cancel'])
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
