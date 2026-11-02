import { isKitClientError } from '../index'
import type { Conversation, RunRequest, Turn } from '../index'

export function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected fixture is missing')
  return value
}

export const model = { endpointId: 'endpoint', modelId: 'model' }

export const request: RunRequest = {
  purpose: 'chat',
  skills: [],
  input: 'Question',
  regenerate: false,
  language: 'en',
  model,
}

export const turn = (id: string, overrides: Partial<Turn> = {}): Turn => ({
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
  contextUsage: { tokens: 160, contextWindow: 1000, source: 'reported', ...model },
  ...overrides,
})

export const stored = (
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

export async function code(promise: Promise<unknown>) {
  const error: unknown = await promise.catch((caught: unknown) => caught)
  return isKitClientError(error) ? error.code : null
}
