// In-memory RunClient, ConversationClient and desktop actions for tests and demos (docs/16, docs/22).
// Seed storage with initialConversations; override clients independently or omit a capability.
import { KitClientError } from './contract'
import type {
  ConversationClient,
  DesktopActions,
  RunClient,
  KitPorts,
  LogsPort,
  McpClientPort,
  McpServerPort,
  ProvidersPort,
  SkillsPort,
} from './contract'
import type { Conversation, LogLevel, RunEvent, RunSnapshot, Turn } from '@ouraihub/aether-contracts'
import type { SearchPort } from './search'

type PortName = keyof KitPorts

export interface FakePortsOptions {
  /** Initial stored conversations, copied to isolate the caller from fake storage. */
  initialConversations?: readonly Conversation[]
  conversations?: Partial<ConversationClient>
  desktop?: Partial<DesktopActions>
  logs?: Partial<LogsPort>
  providers?: Partial<ProvidersPort>
  chat?: Partial<RunClient>
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
    (options.initialConversations ?? []).map((conversation) => [
      conversation.summary.id,
      structuredClone(conversation),
    ]),
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
  const storesConversations = !options.omit?.includes('conversations')
  const runs = new Map<string, RunSnapshot>()
  const terminals: string[] = []
  const acceptRun = (
    conversation: Conversation | null,
    turn: Turn | null,
    select: boolean,
    onEvent: (event: RunEvent) => void,
    signal?: AbortSignal,
  ) => {
    const run: RunSnapshot = {
      runId: nextId('fake-run'),
      conversationId: conversation?.summary.id ?? null,
      turnId: turn?.id ?? null,
      status: 'running',
      cancelRequested: false,
    }
    runs.set(run.runId, run)
    const cancel = () => {
      run.cancelRequested = true
    }
    signal?.addEventListener('abort', cancel, { once: true })
    // Leave an acceptance turn for callers to request cancellation or inspect the active run.
    queueMicrotask(() =>
      queueMicrotask(() => {
        if (signal?.aborted) cancel()
        if (turn && !run.cancelRequested) onEvent({ kind: 'text_delta', text: turn.text })
        run.status = run.cancelRequested ? 'cancelled' : 'completed'
        if (turn && conversation) {
          turn.status = run.status
          if (run.status === 'cancelled') turn.text = ''
          if (select && run.status === 'completed')
            conversation.summary = { ...conversation.summary, selectedTurnId: turn.id, updatedAt: Date.now() }
        }
        signal?.removeEventListener('abort', cancel)
        terminals.push(run.runId)
        if (terminals.length > 128) {
          const expired = terminals.shift()
          if (expired !== undefined) runs.delete(expired)
        }
        onEvent({ kind: 'end', status: run.status })
      }),
    )
    return { runId: run.runId, conversationId: run.conversationId, turnId: run.turnId }
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
  const conversationClient: ConversationClient = {
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
      // Same rule as both hosts (AutoNote docs/35 §3.3): reparent onto the successor, then delete.
      const conversation = getConversation(conversationId)
      const turn = getTurn(conversationId, turnId)
      if (turn.status === 'running') throw clientError('conflict', 'turn running', { reason: 'turn_running' })
      // Turns are stored in creation order, which stands in for `seq`.
      const at = conversation.turns.indexOf(turn)
      const group = conversation.turns.filter(
        (t) => t.kind === 'reply' && t.questionId === turn.questionId && t !== turn,
      )
      const sibling = group.find((t) => conversation.turns.indexOf(t) > at) ?? group.at(-1)
      const successor = sibling?.id ?? turn.parentId
      const removed = new Set([turnId])
      for (const t of conversation.turns)
        if (t.kind === 'translation' && removed.has(t.parentId ?? '')) removed.add(t.id)
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
        let head = successor ?? rest.filter((t) => t.parentId === null).at(-1)?.id ?? null
        for (;;) {
          const child = head === null ? undefined : rest.filter((t) => t.parentId === head).at(-1)
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
      getLevel: () => Promise.resolve({ level, available: true }),
      setLevel: (next) => {
        level = next
        return Promise.resolve({ level, available: true })
      },
      ...options.logs,
    },
    desktop: {
      openLogsDirectory: () => Promise.resolve(),
      ...options.desktop,
    },
    conversations: { ...conversationClient, ...options.conversations },
    providers: {
      revealNeedsPassword: false,
      keyStrategies: false,
      revealKey: () => Promise.reject(clientError('internal', 'Configure the fake provider reveal method')),
      list: () => Promise.resolve([]),
      setEnabled: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      saveKey: () => Promise.resolve([]),
      removeKey: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      modelOptions: () => Promise.resolve([]),
      probe: () => Promise.resolve({ probeUrl: '', models: [], error: null, suggestedProtocol: null }),
      ...options.providers,
    },
    chat: {
      start: async (request, onEvent, callOptions) => {
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
        if ((retry || translation || request.replyToId != null) && (!request.conversationId || !storesConversations))
          throw clientError('bad_request', 'conversation required', { field: 'conversationId' })
        const language = request.targetLanguage?.trim() ?? ''
        if (translation && (!language || Array.from(language).length > 80))
          throw clientError('bad_request', 'invalid language', { field: 'targetLanguage' })
        if (!retry && !translation && !request.input.trim())
          throw clientError('bad_request', 'empty input', { field: 'input' })
        if (!storesConversations || request.purpose === 'writing')
          return acceptRun(null, null, false, onEvent, callOptions?.signal)
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
        return acceptRun(conversation, turn, !retry, onEvent, callOptions?.signal)
      },
      cancel: async (runId) => {
        const run = runs.get(runId)
        if (run?.status === 'running') run.cancelRequested = true
      },
      approve: async (runId) => {
        if (!runs.has(runId)) throw clientError('not_found', `run ${runId}`, { entity: 'run' })
        throw clientError('conflict', 'no pending approval', { reason: 'approval_expired' })
      },
      getSnapshot: async (runId) => {
        const run = runs.get(runId)
        if (!run) throw clientError('not_found', `run ${runId}`, { entity: 'run' })
        return { ...run }
      },
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
