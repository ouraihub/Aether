import { describe, expect, it } from 'vitest'
import { createFakePorts, isKitClientError } from './index'
import { required, stored, turn } from './test-support/fake-fixture'

describe('createFakePorts shape', () => {
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
            { id: 'p', name: 'P', enabled: true, notes: '', presetId: null, endpoints: [] },
          ]),
      },
    })
    expect(await ports.providers?.list()).toEqual([
      { id: 'p', name: 'P', enabled: true, notes: '', presetId: null, endpoints: [] },
    ])
    expect(await ports.providers?.modelOptions()).toEqual([])
  })

  it('overrides conversation and desktop methods independently of chat and logs', async () => {
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

  it('skills.get rejects with not_found and its details', async () => {
    const error: unknown = await createFakePorts().skills?.get('s1').catch(
      (caught: unknown) => caught,
    )
    expect(isKitClientError(error) && [error.code, error.details.entity]).toEqual([
      'not_found',
      'skill',
    ])
  })
})
