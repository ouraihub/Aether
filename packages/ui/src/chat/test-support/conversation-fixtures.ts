import type { Conversation } from '../../ports'
import { model } from './chat-fixture'

export function candidates(): Conversation {
  const reply = (id: string, text: string): Conversation['turns'][number] => ({
    id,
    parentId: null,
    questionId: 'q',
    supersedesId: null,
    kind: 'reply',
    ...model,
    input: 'Shared question',
    text,
    status: 'completed',
    errorCode: null,
    reasoningEffort: 'default',
    reasoningText: null,
    images: [],
    generated: [],
  })
  return {
    summary: { id: 'saved', title: 'Candidates', scope: null, selectedTurnId: 'a', createdAt: 0, updatedAt: 0 },
    turns: [reply('a', 'Original **reply**'), reply('b', 'Alternative reply')],
  }
}
