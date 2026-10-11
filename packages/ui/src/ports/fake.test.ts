import { describe, expect, it } from 'vitest'
import { createFakePorts, isKitClientError } from './index'
import type { RunEvent } from './index'

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
      providers: { list: () => Promise.resolve([{ id: 'p', name: 'P', endpoints: [] }]) },
    })
    expect(await ports.providers?.list()).toEqual([{ id: 'p', name: 'P', endpoints: [] }])
    expect(await ports.providers?.modelOptions()).toEqual([])
  })

  it('chat.run resolves on acceptance and emits exactly one end afterwards', async () => {
    const events: RunEvent[] = []
    const run = createFakePorts().chat?.run(
      { purpose: 'chat', skills: [], input: 'hi', regenerate: false, language: 'en' },
      (event) => events.push(event),
    )
    expect(events).toEqual([])
    expect(await run).toEqual({ runId: 'fake-run', conversationId: null })
    await Promise.resolve()
    expect(events).toEqual([{ kind: 'end', status: 'completed' }])
  })

  it('chat.run with an aborted signal rejects with cancelled', async () => {
    const error: unknown = await createFakePorts()
      .chat?.run({ purpose: 'chat', skills: [], input: 'hi', regenerate: false, language: 'en' }, () => undefined, {
        signal: AbortSignal.abort(),
      })
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && error.code).toBe('cancelled')
  })

  it('skills.get rejects with not_found and its details', async () => {
    const error: unknown = await createFakePorts()
      .skills?.get('s1')
      .catch((caught: unknown) => caught)
    expect(isKitClientError(error) && [error.code, error.details.entity]).toEqual(['not_found', 'skill'])
  })
})
