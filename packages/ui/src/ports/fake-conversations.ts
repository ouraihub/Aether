import type { ConversationClient } from './contract'
import type { Conversation, Turn } from '@ouraihub/aether-contracts'
import { KitClientError } from './contract'

type Context = {
  conversations: Map<string, Conversation>
  nextId: (prefix: string) => string
  nextConversationId: () => string
  getConversation: (id: string) => Conversation
  getTurn: (conversationId: string, turnId: string) => Turn
  snapshot: (conversation: Conversation) => Conversation
}

type ClientError = (
  code: ConstructorParameters<typeof KitClientError>[0]['code'],
  message: string,
  details?: Record<string, string>,
) => KitClientError

export function createFakeConversationClient(
  context: Context,
  clientError: ClientError,
): ConversationClient {
  const { conversations, nextId, getConversation, getTurn, snapshot } = context
  const { nextConversationId } = context
  const conversationClient: ConversationClient = {
    list: (scope?: string) =>
      Promise.resolve(
        [...conversations.values()]
          .filter(({ summary }) => !scope || summary.scope === scope)
          .map(({ summary }) => ({ ...summary })),
      ),
    get: async (id: string) => {
      const conversation = conversations.get(id)
      if (!conversation)
        throw clientError('not_found', `conversation ${id}`, {
          entity: 'conversation',
        })
      return snapshot(conversation)
    },
    remove: async (id: string) => {
      getConversation(id)
      conversations.delete(id)
    },
    rename: async (id: string, title: string) => {
      const conversation = getConversation(id)
      conversation.summary = {
        ...conversation.summary,
        title,
        updatedAt: Date.now(),
      }
      return { ...conversation.summary }
    },
    editTurn: async (
      conversationId: string,
      turnId: string,
      text: string,
      expectedText: string,
    ) => {
      const conversation = getConversation(conversationId)
      const turn = getTurn(conversationId, turnId)
      if (turn.text !== expectedText)
        throw clientError('conflict', 'turn changed', {
          reason: 'text_changed',
        })
      if (!text.trim() || Array.from(text).length > 24_000)
        throw clientError('bad_request', 'invalid text', { field: 'text' })
      if (
        conversation.turns.some(
          (candidate) =>
            candidate.parentId === turnId || candidate.supersedesId === turnId,
        )
      )
        throw clientError('conflict', 'turn has children', {
          reason: 'has_children',
        })
      turn.text = text
      turn.reasoningText = null
      turn.contextUsage = null
      conversation.summary.updatedAt = Date.now()
      return snapshot(conversation)
    },
    removeTurn: async (conversationId: string, turnId: string) => {
      // Reparent onto the successor, then delete (docs/35 §3.3).
      const conversation = getConversation(conversationId)
      const turn = getTurn(conversationId, turnId)
      if (turn.status === 'running')
        throw clientError('conflict', 'turn running', {
          reason: 'turn_running',
        })
      // Turns are stored in creation order, which stands in for `seq`.
      const at = conversation.turns.indexOf(turn)
      const group = conversation.turns.filter(
        (t) =>
          t.kind === 'reply' && t.questionId === turn.questionId && t !== turn,
      )
      const sibling =
        group.find((t) => conversation.turns.indexOf(t) > at) ?? group.at(-1)
      const successor = sibling?.id ?? turn.parentId
      const removed = new Set([turnId])
      for (const t of conversation.turns)
        if (t.kind === 'translation' && removed.has(t.parentId ?? ''))
          removed.add(t.id)
      conversation.turns = conversation.turns
        .filter((t) => !removed.has(t.id))
        .map((t) => ({
          ...t,
          parentId: removed.has(t.parentId ?? '') ? successor : t.parentId,
          supersedesId: removed.has(t.supersedesId ?? '')
            ? sibling && sibling.id !== t.id
              ? sibling.id
              : null
            : t.supersedesId,
        }))
      if (removed.has(conversation.summary.selectedTurnId ?? '')) {
        const rest = conversation.turns.filter((t) => t.kind === 'reply')
        let head =
          successor ??
          rest.filter((t) => t.parentId === null).at(-1)?.id ??
          null
        for (;;) {
          const child =
            head === null
              ? undefined
              : rest.filter((t) => t.parentId === head).at(-1)
          if (!child) break
          head = child.id
        }
        conversation.summary.selectedTurnId = head
      }
      conversation.summary.updatedAt = Date.now()
      return snapshot(conversation)
    },
    fork: async (conversationId: string, turnId: string) => {
      const source = getConversation(conversationId)
      const target = getTurn(conversationId, turnId)
      const chain: Turn[] = []
      const seen = new Set<string>()
      let cursor: Turn | undefined = target
      while (cursor) {
        if (seen.has(cursor.id))
          throw clientError('conflict', 'history cycle', {
            reason: 'history_cycle',
          })
        seen.add(cursor.id)
        chain.unshift(cursor)
        if (!cursor.parentId) break
        cursor = source.turns.find((turn) => turn.id === cursor?.parentId)
        if (!cursor)
          throw clientError('not_found', 'history parent', { entity: 'turn' })
      }
      const ids = new Map<string, string>()
      const questionIds = new Map<string, string>()
      const turns = chain.map((turn) => {
        const id = nextId('fake-turn')
        ids.set(turn.id, id)
        let questionId = questionIds.get(turn.questionId)
        if (questionId === undefined) {
          questionId = id
          questionIds.set(turn.questionId, questionId)
        }
        return {
          ...turn,
          id,
          questionId,
          parentId: turn.parentId ? (ids.get(turn.parentId) ?? null) : null,
          supersedesId: turn.supersedesId
            ? (ids.get(turn.supersedesId) ?? null)
            : null,
        }
      })
      const id = nextConversationId()
      const copy: Conversation = {
        summary: {
          ...source.summary,
          id,
          createdAt: Date.now(),
          updatedAt: Date.now(),
          selectedTurnId: ids.get(turnId) ?? null,
        },
        turns: structuredClone(turns),
      }
      conversations.set(id, copy)
      return snapshot(copy)
    },
    selectTurn: async (conversationId: string, turnId: string) => {
      const conversation = getConversation(conversationId)
      getTurn(conversationId, turnId)
      conversation.summary = {
        ...conversation.summary,
        selectedTurnId: turnId,
        updatedAt: Date.now(),
      }
      return snapshot(conversation)
    },
  }
  return conversationClient
}
