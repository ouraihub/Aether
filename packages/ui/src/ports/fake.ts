// Complete in-memory ports for tests and demos. Override any method; omit a port with `omit`.
import { KitClientError } from './contract'
import type { ChatPort, KitPorts, LogsPort, McpClientPort, McpServerPort, ProvidersPort, SkillsPort } from './contract'
import type { Conversation } from './generated/Conversation'
import type { LogLevel } from './generated/LogLevel'
import type { Turn } from './generated/Turn'
import type { SearchPort } from './search'

type PortName = keyof KitPorts

export interface FakePortsOptions {
  /** Initial stored history, copied so tests and demos cannot mutate storage accidentally. */
  conversations?: readonly Conversation[]
  logs?: Partial<LogsPort>
  providers?: Partial<ProvidersPort>
  chat?: Partial<ChatPort>
  search?: Partial<SearchPort>
  skills?: Partial<SkillsPort>
  mcpClient?: Partial<McpClientPort>
  mcpServer?: Partial<McpServerPort>
  /** Ports to leave out, as a host without that feature would. */
  omit?: readonly PortName[]
}

const STOPPED = { running: false, port: 0, url: '', toolCount: 0 }
const clientError = (
  code: ConstructorParameters<typeof KitClientError>[0]['code'],
  message: string,
  details: Record<string, string> = {},
) => new KitClientError({ code, message, details })

export function createFakePorts(options: FakePortsOptions = {}): KitPorts {
  let level: LogLevel = 'INFO'
  let sequence = 0
  let conversationSequence = 0
  const conversations = new Map<string, Conversation>(
    (options.conversations ?? []).map((conversation) => [conversation.summary.id, structuredClone(conversation)]),
  )
  const nextId = (prefix: string): string => {
    let id: string
    do {
      id = `${prefix}-${++sequence}`
    } while (
      [...conversations.values()].some((conversation) =>
        conversation.turns.some((turn) => turn.id === id || turn.questionId === id),
      )
    )
    return id
  }
  const snapshot = (conversation: Conversation): Conversation => structuredClone(conversation)
  const getConversation = (id: string) => {
    const conversation = conversations.get(id)
    if (!conversation) throw clientError('not_found', `conversation ${id}`, { entity: 'conversation' })
    if (conversation.turns.some((turn) => turn.status === 'running'))
      throw clientError('conflict', 'conversation is active', { reason: 'active_run' })
    return conversation
  }
  const getTurn = (conversationId: string, turnId: string) => {
    const turn = getConversation(conversationId).turns.find((candidate) => candidate.id === turnId)
    if (!turn) throw clientError('not_found', `turn ${turnId}`, { entity: 'turn' })
    return turn
  }
  const history = {
    list: (scope?: string) =>
      Promise.resolve(
        [...conversations.values()]
          .filter(({ summary }) => !scope || summary.scope === scope)
          .map(({ summary }) => ({ ...summary })),
      ),
    get: async (id: string) => {
      const conversation = conversations.get(id)
      if (!conversation) throw clientError('not_found', `conversation ${id}`, { entity: 'conversation' })
      return snapshot(conversation)
    },
    remove: async (id: string) => {
      getConversation(id)
      conversations.delete(id)
    },
    rename: async (id: string, title: string) => {
      const conversation = getConversation(id)
      conversation.summary = { ...conversation.summary, title, updatedAt: Date.now() }
      return { ...conversation.summary }
    },
    editTurn: async (conversationId: string, turnId: string, text: string, expectedText: string) => {
      const conversation = getConversation(conversationId)
      const turn = getTurn(conversationId, turnId)
      if (turn.text !== expectedText) throw clientError('conflict', 'turn changed', { reason: 'text_changed' })
      if (!text.trim() || Array.from(text).length > 24_000)
        throw clientError('bad_request', 'invalid text', { field: 'text' })
      if (conversation.turns.some((candidate) => candidate.parentId === turnId || candidate.supersedesId === turnId))
        throw clientError('conflict', 'turn has children', { reason: 'has_children' })
      turn.text = text
      turn.reasoningText = null
      turn.contextUsage = null
      conversation.summary.updatedAt = Date.now()
      return snapshot(conversation)
    },
    removeTurn: async (conversationId: string, turnId: string) => {
      const conversation = getConversation(conversationId)
      const turn = getTurn(conversationId, turnId)
      if (conversation.summary.selectedTurnId === turnId)
        throw clientError('conflict', 'selected turn', { reason: 'selected_turn' })
      if (conversation.turns.some((candidate) => candidate.parentId === turnId || candidate.supersedesId === turnId))
        throw clientError('conflict', 'turn has children', { reason: 'has_children' })
      conversation.turns = conversation.turns.filter((candidate) => candidate !== turn)
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
        if (seen.has(cursor.id)) throw clientError('conflict', 'history cycle', { reason: 'history_cycle' })
        seen.add(cursor.id)
        chain.unshift(cursor)
        if (!cursor.parentId) break
        cursor = source.turns.find((turn) => turn.id === cursor?.parentId)
        if (!cursor) throw clientError('not_found', 'history parent', { entity: 'turn' })
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
          supersedesId: turn.supersedesId ? (ids.get(turn.supersedesId) ?? null) : null,
        }
      })
      let id: string
      do {
        id = `fake-conversation-${++conversationSequence}`
      } while (conversations.has(id))
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
      conversation.summary = { ...conversation.summary, selectedTurnId: turnId, updatedAt: Date.now() }
      return snapshot(conversation)
    },
  }
  const ports: Required<Omit<KitPorts, 'search'>> = {
    logs: {
      tail: () => Promise.resolve([]),
      ingest: (batch) => Promise.resolve({ accepted: batch.entries.length, rejected: 0 }),
      openDir: () => Promise.resolve(),
      getLevel: () => Promise.resolve({ level, available: true }),
      setLevel: (next) => {
        level = next
        return Promise.resolve({ level, available: true })
      },
      ...options.logs,
    },
    providers: {
      list: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      modelOptions: () => Promise.resolve([]),
      probe: () => Promise.resolve({ probeUrl: '', models: [], error: null }),
      ...options.providers,
    },
    chat: {
      run: async (request, onEvent, callOptions) => {
        if (callOptions?.signal?.aborted) throw clientError('cancelled', 'cancelled')
        const retry = request.retryTurnId != null || request.regenerate
        const translation = request.translateTurnId != null
        const action = request.retryTurnId != null || translation
        if (
          (retry && translation) ||
          (action && (request.regenerate || request.webSearch === true)) ||
          (request.replyToId != null && (retry || translation)) ||
          (request.targetLanguage != null && !translation)
        )
          throw clientError('bad_request', 'invalid graph request', { field: 'action' })
        if (request.images?.length && (retry || translation))
          throw clientError('bad_request', 'images on action', { field: 'images', reason: 'action_conflict' })
        if (request.purpose === 'writing' && (request.replyToId != null || action || request.targetLanguage != null))
          throw clientError('bad_request', 'writing graph fields', { field: 'action' })
        if ((retry || translation || request.replyToId != null) && (!request.conversationId || !ports.chat.history))
          throw clientError('bad_request', 'conversation required', { field: 'conversationId' })
        const language = request.targetLanguage?.trim() ?? ''
        if (translation && (!language || Array.from(language).length > 80))
          throw clientError('bad_request', 'invalid language', { field: 'targetLanguage' })
        if (!retry && !translation && !request.input.trim())
          throw clientError('bad_request', 'empty input', { field: 'input' })
        if (!ports.chat.history || request.purpose === 'writing') {
          queueMicrotask(() => onEvent({ kind: 'end', status: 'completed' }))
          return { runId: 'fake-run', conversationId: null, turnId: null }
        }
        let conversationId = request.conversationId
        if (!conversationId) {
          do {
            conversationId = `fake-conversation-${++conversationSequence}`
          } while (conversations.has(conversationId))
        }
        const conversation: Conversation = request.conversationId
          ? getConversation(conversationId)
          : {
              summary: {
                id: conversationId,
                title: request.input.slice(0, 80),
                scope: request.scope ?? null,
                createdAt: Date.now(),
                updatedAt: Date.now(),
                selectedTurnId: null,
              },
              turns: [],
            }
        const selectedId =
          conversation.summary.selectedTurnId ??
          [...conversation.turns].reverse().find((turn) => turn.status === 'completed')?.id ??
          null
        const sourceId = request.retryTurnId ?? request.translateTurnId ?? (retry ? selectedId : null)
        const source = sourceId ? getTurn(conversationId, sourceId) : undefined
        if ((retry || translation) && !source) throw clientError('not_found', 'source turn', { entity: 'turn' })
        if (request.replyToId) getTurn(conversationId, request.replyToId)
        if (!retry && !translation && selectedId && !request.replyToId) getTurn(conversationId, selectedId)
        const model =
          request.model ??
          (source?.endpointId && source.modelId ? { endpointId: source.endpointId, modelId: source.modelId } : null)
        if (source && !model) throw clientError('needs_config', 'source model unavailable')
        if (source && model && options.providers?.modelOptions) {
          const models = await ports.providers.modelOptions()
          if (!models.some((option) => option.endpointId === model.endpointId && option.modelId === model.modelId))
            throw clientError('needs_config', 'source model unavailable')
        }
        if (callOptions?.signal?.aborted) throw clientError('cancelled', 'cancelled')
        if (request.conversationId) getConversation(conversationId)
        conversations.set(conversationId, conversation)
        const turnId = nextId('fake-turn')
        const translationSource = translation ? source : undefined
        const retrySource = retry ? source : undefined
        const input = translationSource
          ? `Translate the following reply into ${language}. Return only the translation, preserving Markdown formatting.\n\n${translationSource.text}`
          : retrySource
            ? retrySource.input
            : request.input
        const turn: Turn = {
          id: turnId,
          parentId: translationSource
            ? translationSource.id
            : retrySource
              ? retrySource.parentId
              : (request.replyToId ?? selectedId),
          questionId: retrySource ? retrySource.questionId : turnId,
          supersedesId: retrySource ? retrySource.id : null,
          kind: translationSource ? 'translation' : retrySource ? retrySource.kind : 'reply',
          endpointId: model?.endpointId ?? null,
          modelId: model?.modelId ?? null,
          input,
          images: translationSource
            ? []
            : retrySource
              ? retrySource.images
              : (request.images ?? []).map((image) => ({
                  mime: image.mime,
                  url: `data:${image.mime};base64,${image.data}`,
                })),
          generated: [],
          text: `Fake reply: ${input}`,
          status: 'running',
          errorCode: null,
          reasoningText: null,
          reasoningEffort: request.reasoningEffort ?? source?.reasoningEffort ?? 'default',
          usage: null,
          contextUsage: {
            tokens: input.length * 2 + 12,
            contextWindow: null,
            source: 'estimated',
            endpointId: model?.endpointId ?? null,
            modelId: model?.modelId ?? null,
          },
        }
        conversation.turns.push(turn)
        queueMicrotask(() => {
          turn.status = callOptions?.signal?.aborted ? 'cancelled' : 'completed'
          if (!retry && turn.status === 'completed')
            conversation.summary = { ...conversation.summary, selectedTurnId: turnId, updatedAt: Date.now() }
          if (turn.status === 'completed') onEvent({ kind: 'text_delta', text: turn.text })
          else turn.text = ''
          onEvent({ kind: 'end', status: turn.status })
        })
        return { runId: nextId('fake-run'), conversationId, turnId }
      },
      approve: () => Promise.resolve(),
      history,
      ...options.chat,
    },
    skills: {
      list: () => Promise.resolve([]),
      get: (id) => Promise.reject(clientError('not_found', `skill ${id}`, { entity: 'skill' })),
      preview: () => Promise.reject(clientError('bad_request', 'no package', { field: 'SKILL.md' })),
      install: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      duplicate: () => Promise.resolve([]),
      reorder: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      ...options.skills,
    },
    mcpClient: {
      list: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      refresh: () => Promise.resolve([]),
      setToolApproval: () => Promise.resolve(),
      ...options.mcpClient,
    },
    mcpServer: {
      status: () => Promise.resolve(STOPPED),
      tokens: () => Promise.resolve([]),
      start: () => Promise.resolve(STOPPED),
      stop: () => Promise.resolve(STOPPED),
      setPort: (port) => Promise.resolve({ ...STOPPED, port }),
      issueToken: (name, scopes) =>
        Promise.resolve({
          token: 'kit_fake',
          id: 'fake-token',
          name,
          scopes: [...scopes],
          createdAt: 0,
          lastUsedAt: null,
          revokedAt: null,
        }),
      revokeToken: () => Promise.resolve(),
      scopes: () => Promise.resolve([]),
      setScopes: () => Promise.resolve([]),
      ...options.mcpServer,
    },
  }
  const result: KitPorts = { ...ports }
  if (options.search) {
    const unavailable = () => Promise.reject(clientError('internal', 'Configure the fake search method'))
    result.search = {
      settings: unavailable,
      save: unavailable,
      setDefault: unavailable,
      savePreferences: unavailable,
      saveKey: unavailable,
      removeKey: unavailable,
      reveal: unavailable,
      check: unavailable,
      search: unavailable,
      fetch: unavailable,
      ...options.search,
    }
  }
  for (const name of options.omit ?? []) delete result[name]
  return result
}
