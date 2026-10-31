import { describe, expect, it } from 'vitest'
import { createFakePorts, isKitClientError } from './index'
import type { Conversation, RunRequest, Turn } from './index'

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
const turn = (id: string): Turn => ({
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
  usage: null,
  contextUsage: null,
})
const stored = (turns: Turn[]): Conversation => ({
  summary: {
    id: 'stored',
    title: 'Stored',
    scope: 'scope',
    createdAt: 1,
    updatedAt: 2,
    selectedTurnId: turns[0]?.id ?? null,
  },
  turns,
})

async function code(promise: Promise<unknown>) {
  const error: unknown = await promise.catch((caught: unknown) => caught)
  return isKitClientError(error) ? error.code : null
}

describe('createFakePorts invalid actions', () => {
  it('rejects invalid actions and missing source models', async () => {
    const ports = createFakePorts({
      initialConversations: [stored([turn('source')])],
    })
    const chat = required(ports.chat)
    const invalid: Partial<RunRequest>[] = [
      {
        retryTurnId: 'source',
        translateTurnId: 'source',
        targetLanguage: 'French',
      },
      { retryTurnId: 'source', regenerate: true },
      { translateTurnId: 'source', regenerate: true },
      { retryTurnId: 'source', webSearch: true },
      { translateTurnId: 'source' },
      { targetLanguage: 'French' },
      { replyToId: 'source', retryTurnId: 'source' },
      { purpose: 'writing', replyToId: 'source' },
      { purpose: 'writing', retryTurnId: 'source' },
      {
        purpose: 'writing',
        translateTurnId: 'source',
        targetLanguage: 'French',
      },
    ]
    for (const fields of invalid)
      expect(
        await code(
          chat.start(
            { ...request, conversationId: 'stored', ...fields },
            () => undefined,
          ),
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
          {
            ...request,
            model: null,
            conversationId: 'stored',
            retryTurnId: 'source',
          },
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
      await code(
        chat.start(
          { ...request, conversationId: 'stored', replyToId: 'missing' },
          () => undefined,
        ),
      ),
    ).toBe('not_found')
    expect(
      await code(
        chat.start({ ...request, conversationId: 'missing' }, () => undefined),
      ),
    ).toBe('not_found')
    expect(
      (await required(ports.conversations).get('stored')).turns,
    ).toHaveLength(1)
    expect(
      await code(
        chat.start({ ...request, retryTurnId: 'source' }, () => undefined),
      ),
    ).toBe('bad_request')
  })
})
