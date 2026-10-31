// In-memory RunClient, ConversationClient and desktop actions for tests.
// Seed storage with initialConversations; override clients or omit features.
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
import type {
  Conversation,
  LogLevel,
  RunEvent,
  RunSnapshot,
  Turn,
} from '@ouraihub/aether-contracts'
import type { SearchPort } from './search'
import { createFakeConversationClient } from './fake-conversations'
import type { SkillSourcesPort } from './skill-sources'

type PortName = keyof KitPorts

export interface FakePortsOptions {
  /** Initial stored conversations, copied to isolate fake storage. */
  initialConversations?: readonly Conversation[]
  conversations?: Partial<ConversationClient>
  desktop?: Partial<DesktopActions>
  logs?: Partial<LogsPort>
  providers?: Partial<ProvidersPort>
  chat?: Partial<RunClient>
  search?: Partial<SearchPort>
  skills?: Partial<SkillsPort>
  skillSources?: Partial<SkillSourcesPort>
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
        conversation.turns.some(
          (turn) => turn.id === id || turn.questionId === id,
        ),
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
    // Leave an acceptance turn for callers to inspect or request cancellation.
    queueMicrotask(() =>
      queueMicrotask(() => {
        if (signal?.aborted) cancel()
        if (turn && !run.cancelRequested)
          onEvent({ kind: 'text_delta', text: turn.text })
        run.status = run.cancelRequested ? 'cancelled' : 'completed'
        if (turn && conversation) {
          turn.status = run.status
          if (run.status === 'cancelled') turn.text = ''
          if (select && run.status === 'completed')
            conversation.summary = {
              ...conversation.summary,
              selectedTurnId: turn.id,
              updatedAt: Date.now(),
            }
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
    return {
      runId: run.runId,
      conversationId: run.conversationId,
      turnId: run.turnId,
    }
  }
  const snapshot = (conversation: Conversation): Conversation =>
    structuredClone(conversation)
  const getConversation = (id: string) => {
    const conversation = conversations.get(id)
    if (!conversation)
      throw clientError('not_found', `conversation ${id}`, {
        entity: 'conversation',
      })
    if (conversation.turns.some((turn) => turn.status === 'running'))
      throw clientError('conflict', 'conversation is active', {
        reason: 'active_run',
      })
    return conversation
  }
  const getTurn = (conversationId: string, turnId: string) => {
    const turn = getConversation(conversationId).turns.find(
      (candidate) => candidate.id === turnId,
    )
    if (!turn)
      throw clientError('not_found', `turn ${turnId}`, { entity: 'turn' })
    return turn
  }
  const nextConversationId = () => {
    let id: string
    do id = `fake-conversation-${++conversationSequence}`
    while (conversations.has(id))
    return id
  }
  const conversationClient = createFakeConversationClient(
    { conversations, nextId, nextConversationId, getConversation, getTurn,
      snapshot },
    clientError,
  )
  const ports: Required<Omit<KitPorts, 'search' | 'skillSources'>> = {
    logs: {
      tail: () => Promise.resolve([]),
      ingest: (batch) =>
        Promise.resolve({ accepted: batch.entries.length, rejected: 0 }),
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
      revealKey: () =>
        Promise.reject(
          clientError('internal', 'Configure the fake provider reveal method'),
        ),
      list: () => Promise.resolve([]),
      setEnabled: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      saveKey: () => Promise.resolve([]),
      removeKey: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      modelOptions: () => Promise.resolve([]),
      probe: () =>
        Promise.resolve({
          probeUrl: '',
          models: [],
          error: null,
          suggestedProtocol: null,
        }),
      ...options.providers,
    },
    chat: {
      start: async (request, onEvent, callOptions) => {
        if (callOptions?.signal?.aborted)
          throw clientError('cancelled', 'cancelled')
        const retry = request.retryTurnId != null || request.regenerate
        const translation = request.translateTurnId != null
        const action = request.retryTurnId != null || translation
        if (
          (retry && translation) ||
          (action && (request.regenerate || request.webSearch === true)) ||
          (request.replyToId != null && (retry || translation)) ||
          (request.targetLanguage != null && !translation)
        )
          throw clientError('bad_request', 'invalid graph request', {
            field: 'action',
          })
        if (request.images?.length && (retry || translation))
          throw clientError('bad_request', 'images on action', {
            field: 'images',
            reason: 'action_conflict',
          })
        if (
          request.purpose === 'writing' &&
          (request.replyToId != null ||
            action ||
            request.targetLanguage != null)
        )
          throw clientError('bad_request', 'writing graph fields', {
            field: 'action',
          })
        if (
          (retry || translation || request.replyToId != null) &&
          (!request.conversationId || !storesConversations)
        )
          throw clientError('bad_request', 'conversation required', {
            field: 'conversationId',
          })
        const language = request.targetLanguage?.trim() ?? ''
        if (translation && (!language || Array.from(language).length > 80))
          throw clientError('bad_request', 'invalid language', {
            field: 'targetLanguage',
          })
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
          [...conversation.turns]
            .reverse()
            .find((turn) => turn.status === 'completed')?.id ??
          null
        const sourceId =
          request.retryTurnId ??
          request.translateTurnId ??
          (retry ? selectedId : null)
        const source = sourceId ? getTurn(conversationId, sourceId) : undefined
        if ((retry || translation) && !source)
          throw clientError('not_found', 'source turn', { entity: 'turn' })
        if (request.replyToId) getTurn(conversationId, request.replyToId)
        if (!retry && !translation && selectedId && !request.replyToId)
          getTurn(conversationId, selectedId)
        const model =
          request.model ??
          (source?.endpointId && source.modelId
            ? { endpointId: source.endpointId, modelId: source.modelId }
            : null)
        if (source && !model)
          throw clientError('needs_config', 'source model unavailable')
        if (source && model && options.providers?.modelOptions) {
          const models = await ports.providers.modelOptions()
          if (
            !models.some(
              (option) =>
                option.endpointId === model.endpointId &&
                option.modelId === model.modelId,
            )
          )
            throw clientError('needs_config', 'source model unavailable')
        }
        if (callOptions?.signal?.aborted)
          throw clientError('cancelled', 'cancelled')
        if (request.conversationId) getConversation(conversationId)
        conversations.set(conversationId, conversation)
        const turnId = nextId('fake-turn')
        const translationSource = translation ? source : undefined
        const retrySource = retry ? source : undefined
        const input = translationSource
          ? 'Translate the following reply into '
            + `${language}. Return only the translation, preserving Markdown`
            + ` formatting.`
            + `\n\n${translationSource.text}`
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
          kind: translationSource
            ? 'translation'
            : retrySource
              ? retrySource.kind
              : 'reply',
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
          reasoningEffort:
            request.reasoningEffort ?? source?.reasoningEffort ?? 'default',
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
        return acceptRun(
          conversation,
          turn,
          !retry,
          onEvent,
          callOptions?.signal,
        )
      },
      cancel: async (runId) => {
        const run = runs.get(runId)
        if (run?.status === 'running') run.cancelRequested = true
      },
      approve: async (runId) => {
        if (!runs.has(runId))
          throw clientError('not_found', `run ${runId}`, { entity: 'run' })
        throw clientError('conflict', 'no pending approval', {
          reason: 'approval_expired',
        })
      },
      getSnapshot: async (runId) => {
        const run = runs.get(runId)
        if (!run)
          throw clientError('not_found', `run ${runId}`, { entity: 'run' })
        return { ...run }
      },
      ...options.chat,
    },
    skills: {
      list: () => Promise.resolve([]),
      get: (id) =>
        Promise.reject(
          clientError('not_found', `skill ${id}`, { entity: 'skill' }),
        ),
      preview: () =>
        Promise.reject(
          clientError('bad_request', 'no package', { field: 'SKILL.md' }),
        ),
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
    const unavailable = () =>
      Promise.reject(
        clientError('internal', 'Configure the fake search method'),
      )
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
  if (options.skillSources) {
    const unavailable = () =>
      Promise.reject(
        clientError('internal', 'Configure the fake skill source method'),
      )
    result.skillSources = {
      search: unavailable,
      prepare: unavailable,
      discover: unavailable,
      prepareSystem: unavailable,
      pickDirectory: unavailable,
      pickSearchDirectory: unavailable,
      ...options.skillSources,
    }
  }
  for (const name of options.omit ?? []) delete result[name]
  return result
}
