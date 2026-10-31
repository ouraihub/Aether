// One conversation in the panel: turns, the active run and its abort (docs/16 §4.2, §7 trap 4).
import { SvelteMap } from 'svelte/reactivity'
import { isKitClientError } from '../ports'
import type {
  RunClient,
  ConversationClient,
  ContextRef,
  ImagePart,
  ImageRef,
  KitError,
  ModelRef,
  RunEvent,
  RunRequest,
  SearchHit,
  SkillRef,
  ToolOutcome,
  Turn,
  TurnStatus,
} from '../ports'

/** One tool call of a turn, as shown by ToolCallItem / ApprovalCard. */
export interface ToolItem {
  callId: string
  name: string
  /** MCP server name; absent for host tools. */
  server: string | null
  /** Arguments preview while the call waits for the user. */
  pending: string | null
  outcome: ToolOutcome | null
  summary: string
}

import type {
  ContextUsage,
  Cost,
  TokenUsage,
  ReasoningEffort,
  PermissionMode,
} from '../ports'

export interface ChatTurn {
  id: string
  persisted: boolean
  parentId: string | null
  questionId: string
  supersedesId: string | null
  kind: 'reply' | 'translation'
  endpointId: string | null
  modelId: string | null
  input: string
  images: ImageRef[]
  /** Images `generate_image` produced in this reply (docs/20). */
  generated: ImageRef[]
  text: string
  reasoningText: string
  reasoningEffort: ReasoningEffort
  usage: TokenUsage | null
  /** Priced at run time; later price changes never recompute it (docs/41 §3.7). */
  cost: Cost | null
  contextUsage: ContextUsage | null
  status: TurnStatus
  error: string | null
  tools: ToolItem[]
  exhausted: boolean
  search: SearchState
}

export interface SearchState {
  status: 'idle' | 'searching' | 'complete' | 'failed'
  query: string | null
  provider: string | null
  hits: SearchHit[]
  error: string | null
}

export interface RunBase {
  purpose: string
  scope: string | null
  language: string
  model: ModelRef | null
  skills: SkillRef[]
  context: ContextRef | null
  reasoningEffort?: ReasoningEffort
  /** Offer the `generate_image` tool (docs/20). */
  generateImage?: boolean
  /** Permission mode of the run (docs/16 §4.2); omitted is `default`. */
  permissionMode?: PermissionMode
}

export class RunSession {
  turns = $state<ChatTurn[]>([])
  conversationId = $state<string | null>(null)
  selectedTurnId = $state<string | null>(null)
  actionError = $state<string | null>(null)
  historyRefreshError = $state<string | null>(null)
  running = $state(false)
  contextUsage = $state<ContextUsage | null>(null)
  search = $state<SearchState>({
    status: 'idle',
    query: null,
    provider: null,
    hits: [],
    error: null,
  })
  /** Id of a selected skill the backend rejected as `skill_changed`; the user has to confirm it again. */
  skillChanged = $state<string | null>(null)
  #abort: AbortController | null = null
  /** Id of the active run, known once `chat_run` resolves. */
  #runId: string | null = null
  readonly #port: RunClient
  readonly #conversations: ConversationClient | undefined
  #ended = false

  constructor(port: RunClient, conversations?: ConversationClient) {
    this.#port = port
    this.#conversations = conversations
  }
  resetContext() {
    this.contextUsage = null
  }

  get cumulativeUsage(): TokenUsage {
    const total: TokenUsage = {
      inputTokens: null,
      outputTokens: null,
      cacheReadTokens: null,
      cacheWriteTokens: null,
      reasoningTokens: null,
    }
    for (const turn of this.turns)
      for (const key of Object.keys(total) as (keyof TokenUsage)[]) {
        const value = turn.usage?.[key]
        if (value != null) total[key] = (total[key] ?? 0) + value
      }
    return total
  }

  /** Known turn costs summed per currency, same scope as `cumulativeUsage` (no exchange rates). */
  get cumulativeCost(): Cost[] {
    const totals: Cost[] = []
    for (const { cost } of this.turns) {
      if (!cost) continue
      const found = totals.find((item) => item.currency === cost.currency)
      if (found) found.amount += cost.amount
      else totals.push({ ...cost })
    }
    return totals
  }

  send(
    base: RunBase,
    input: string,
    webSearch = false,
    images: ImagePart[] = [],
  ): Promise<boolean> {
    if (this.running || input.trim() === '') return Promise.resolve(false)
    return this.#append(
      base,
      input,
      this.selectedTurnId,
      null,
      'reply',
      images.length ? { images } : {},
      webSearch,
    )
  }

  retry(
    base: RunBase,
    turnId: string,
    model?: ModelRef | null,
  ): Promise<boolean> {
    if (base.purpose !== 'chat') {
      this.actionError = 'bad_request'
      return Promise.resolve(false)
    }
    const source = this.#source(turnId)
    if (!source) return Promise.resolve(false)
    const snapshot = {
      ...base,
      reasoningEffort: source.reasoningEffort,
      model:
        model ??
        (source.endpointId && source.modelId
          ? { endpointId: source.endpointId, modelId: source.modelId }
          : source.persisted
            ? null
            : base.model),
    }
    const storedSource = this.#stored && source.persisted
    // Without stored history the host cannot read the source images back, so resend the local data URLs.
    const images = source.images.flatMap((image) =>
      image.url?.startsWith('data:')
        ? [
            {
              mime: image.mime,
              data: image.url.slice(image.url.indexOf(',') + 1),
            },
          ]
        : [],
    )
    return this.#append(
      snapshot,
      source.input,
      source.parentId,
      this.#stored && !source.persisted ? null : source,
      source.kind,
      storedSource
        ? { retryTurnId: source.id }
        : images.length
          ? { images }
          : {},
    )
  }

  translate(base: RunBase, turnId: string, language: string): Promise<boolean> {
    if (base.purpose !== 'chat') {
      this.actionError = 'bad_request'
      return Promise.resolve(false)
    }
    const source = this.#source(turnId)
    if (!source) return Promise.resolve(false)
    if (!language.trim() || language.trim().length > 80) {
      this.actionError = 'bad_request'
      return Promise.resolve(false)
    }
    const input = `Translate the following reply into ${language.trim()}. Return only the translation, preserving Markdown formatting.\n\n${source.text}`
    const snapshot = {
      ...base,
      reasoningEffort: source.reasoningEffort,
      model:
        base.model ??
        (source.endpointId && source.modelId
          ? { endpointId: source.endpointId, modelId: source.modelId }
          : null),
    }
    return this.#append(
      snapshot,
      input,
      source.id,
      null,
      'translation',
      this.#stored && source.persisted
        ? { translateTurnId: source.id, targetLanguage: language.trim() }
        : {},
    )
  }

  get #stored() {
    return this.#conversations !== undefined && this.conversationId !== null
  }

  #source(id: string) {
    if (this.running) return null
    const source = this.turns.find((turn) => turn.id === id)
    if (!source) this.actionError = 'not_found'
    return source ?? null
  }

  #append(
    base: RunBase,
    input: string,
    parentId: string | null,
    source: ChatTurn | null,
    kind: ChatTurn['kind'],
    action: Partial<RunRequest>,
    webSearch = false,
  ): Promise<boolean> {
    const id = crypto.randomUUID()
    this.actionError = null
    this.historyRefreshError = null
    this.turns.push({
      id,
      persisted: false,
      parentId,
      questionId: source?.questionId ?? id,
      supersedesId: source?.id ?? null,
      kind,
      endpointId: base.model?.endpointId ?? null,
      modelId: base.model?.modelId ?? null,
      input,
      images:
        action.images?.map((image) => ({
          mime: image.mime,
          url: `data:${image.mime};base64,${image.data}`,
        })) ??
        source?.images ??
        [],
      generated: [],
      text: '',
      reasoningText: '',
      reasoningEffort: base.reasoningEffort ?? 'default',
      usage: null,
      cost: null,
      contextUsage: null,
      status: 'running',
      error: null,
      tools: [],
      exhausted: false,
      search: {
        status: 'idle',
        query: null,
        provider: null,
        hits: [],
        error: null,
      },
    })
    return this.#start(base, input, action, webSearch)
  }

  /** Cancels the active run; its `end` arrives as `cancelled`. */
  stop() {
    this.#abort?.abort()
    for (const turn of this.turns)
      for (const tool of turn.tools) tool.pending = null
    if (this.#runId && !this.#ended) {
      const abort = this.#abort
      void this.#port.cancel(this.#runId).catch((error: unknown) => {
        if (this.#abort === abort && this.running)
          this.actionError = isKitClientError(error) ? error.code : 'internal'
      })
    }
  }

  /** Invalidates callbacks before the owning view is destroyed. */
  dispose() {
    this.stop()
    this.#abort = null
    this.#runId = null
    this.running = false
  }

  /** Answers an `approval_required` call of the active run. */
  approve(tool: ToolItem, allow: boolean) {
    const runId = this.#runId
    const abort = this.#abort
    if (
      tool.pending === null ||
      runId === null ||
      this.#ended ||
      abort?.signal.aborted
    )
      return
    const preview = tool.pending
    tool.pending = null
    this.#port.approve(runId, tool.callId, allow).catch((error: unknown) => {
      if (
        this.#abort !== abort ||
        this.#ended ||
        abort?.signal.aborted ||
        !this.running
      )
        return
      this.actionError = isKitClientError(error) ? error.code : 'internal'
      if (tool.outcome === null) tool.pending = preview
    })
  }

  show(
    conversationId: string | null,
    turns: readonly Turn[] = [],
    selectedTurnId?: string | null,
  ) {
    const previous =
      conversationId === this.conversationId
        ? new SvelteMap(this.turns.map((turn) => [turn.id, turn]))
        : null
    this.stop()
    this.#abort = null
    this.#runId = null
    this.running = false
    this.actionError = null
    this.historyRefreshError = null
    this.conversationId = conversationId
    this.selectedTurnId =
      selectedTurnId === undefined
        ? (turns.findLast((turn) => turn.status === 'completed')?.id ?? null)
        : selectedTurnId
    this.contextUsage =
      turns.find((turn) => turn.id === this.selectedTurnId)?.contextUsage ??
      null
    this.search = {
      status: 'idle',
      query: null,
      provider: null,
      hits: [],
      error: null,
    }
    this.turns = turns.map((turn) => {
      const restored = this.#fromHistory(turn)
      const current = previous?.get(turn.id)
      if (
        current &&
        current.text === restored.text &&
        current.reasoningText === restored.reasoningText &&
        (current.contextUsage === null) === (restored.contextUsage === null)
      ) {
        restored.tools = current.tools
        restored.search = current.search
        restored.exhausted = current.exhausted
      }
      return restored
    })
  }

  #fromHistory(turn: Turn): ChatTurn {
    return {
      id: turn.id,
      persisted: true,
      parentId: turn.parentId,
      questionId: turn.questionId,
      supersedesId: turn.supersedesId,
      kind: turn.kind,
      endpointId: turn.endpointId,
      modelId: turn.modelId,
      input: turn.input,
      images: turn.images,
      generated: turn.generated,
      text: turn.text,
      reasoningText: turn.reasoningText ?? '',
      reasoningEffort: turn.reasoningEffort ?? 'default',
      usage: turn.usage ?? null,
      cost: turn.cost ?? null,
      contextUsage: turn.contextUsage ?? null,
      status: turn.status,
      error: turn.errorCode,
      tools: [],
      exhausted: false,
      search: {
        status: 'idle',
        query: null,
        provider: null,
        hits: [],
        error: null,
      },
    }
  }

  async #start(
    base: RunBase,
    input: string,
    action: Partial<RunRequest>,
    webSearch: boolean,
  ): Promise<boolean> {
    // The proxied element, so streamed writes are reactive.
    const turn = this.turns[this.turns.length - 1]
    if (!turn) return Promise.resolve(false)
    const abort = new AbortController()
    this.#abort = abort
    this.#ended = false
    this.contextUsage = null
    this.search = {
      status: webSearch ? 'searching' : 'idle',
      query: webSearch ? input : null,
      provider: null,
      hits: [],
      error: null,
    }
    turn.search = this.search
    this.running = true
    let accepted = false
    let ended = false
    let refreshing = false
    const settle = async () => {
      if (!accepted || !ended || refreshing || this.#abort !== abort) return
      refreshing = true
      try {
        if (this.#conversations && this.conversationId) {
          const conversation = await this.#conversations.get(
            this.conversationId,
          )
          if (this.#abort !== abort) return
          const live = new SvelteMap(this.turns.map((item) => [item.id, item]))
          this.turns = conversation.turns.map((stored) => {
            const current = live.get(stored.id)
            live.delete(stored.id)
            return {
              ...this.#fromHistory(stored),
              tools: current?.tools ?? [],
              search: current?.search ?? {
                status: 'idle',
                query: null,
                provider: null,
                hits: [],
                error: null,
              },
              exhausted: current?.exhausted ?? false,
            }
          })
          this.turns.push(...live.values())
          this.selectedTurnId = conversation.summary.selectedTurnId
          this.contextUsage =
            this.turns.find((item) => item.id === this.selectedTurnId)
              ?.contextUsage ?? null
        }
      } catch (error: unknown) {
        if (this.#abort !== abort) return
        this.historyRefreshError =
          error instanceof Error ? error.message : 'internal'
      } finally {
        if (this.#abort === abort) {
          this.#abort = null
          this.#runId = null
          this.running = false
        }
      }
    }
    const finish = (status: TurnStatus, error: string | null) => {
      if (this.#abort !== abort) return
      ended = true
      this.#ended = true
      turn.status = status
      turn.error = error
      if (status !== 'completed') {
        turn.contextUsage = null
        this.contextUsage = null
      } else if (turn.supersedesId === null) this.selectedTurnId = turn.id
      if (this.search.status === 'searching') {
        this.search = {
          ...this.search,
          status: status === 'failed' ? 'failed' : 'idle',
          error,
        }
        turn.search = this.search
      }
      void settle()
    }
    const onEvent = (event: RunEvent) => {
      if (this.#abort !== abort || ended) return
      const tool = (callId: string) =>
        turn.tools.find((x) => x.callId === callId)
      switch (event.kind) {
        case 'search_start':
          this.search.status = 'searching'
          break
        case 'search_result':
          this.search = {
            status: 'complete',
            query: event.query,
            provider: event.provider,
            hits: event.hits,
            error: null,
          }
          turn.search = this.search
          break
        case 'text_delta':
          turn.text += event.text
          break
        case 'reasoning_delta':
          turn.reasoningText += event.text
          break
        case 'context':
          turn.contextUsage = event.usage
          this.contextUsage = event.usage
          break
        case 'usage':
          turn.usage = {
            inputTokens: event.inputTokens,
            outputTokens: event.outputTokens,
            cacheReadTokens: event.cacheReadTokens,
            cacheWriteTokens: event.cacheWriteTokens,
            reasoningTokens: event.reasoningTokens,
          }
          turn.cost = event.cost
          break
        case 'tool_call':
          // A repeated call id is fed back without running again (docs/12 §5): one row.
          if (!tool(event.callId)) {
            const server = event.server ?? null
            turn.tools.push({
              callId: event.callId,
              name: event.name,
              server,
              pending: null,
              outcome: null,
              summary: '',
            })
          }
          break
        case 'approval_required': {
          const item = tool(event.callId)
          if (item) item.pending = event.argsPreview
          break
        }
        case 'tool_result': {
          const item = tool(event.callId)
          if (item)
            Object.assign(item, {
              pending: null,
              outcome: event.outcome,
              summary: event.summary,
            })
          const painted = (event.payload as { images?: ImageRef[] } | undefined)
            ?.images
          if (event.outcome === 'ok' && Array.isArray(painted))
            turn.generated.push(...painted)
          break
        }
        case 'end':
          turn.exhausted = event.exhausted === true
          // Calls still waiting when the run stops never ran.
          for (const item of turn.tools) item.pending = null
          this.#checkSkill(event.error)
          finish(event.status, event.error?.code ?? null)
          break
        case 'round_start':
          turn.contextUsage = null
          this.contextUsage = null
          break
      }
    }
    const request: RunRequest = {
      purpose: base.purpose,
      model: base.model,
      skills: base.skills,
      context: base.context,
      input,
      conversationId: this.conversationId,
      scope: base.scope,
      regenerate: false,
      language: base.language,
      reasoningEffort: base.reasoningEffort ?? 'default',
      permissionMode: base.permissionMode ?? 'default',
      ...(base.generateImage && !action.translateTurnId
        ? { generateImage: true }
        : {}),
      ...(base.purpose === 'chat' &&
      this.#stored &&
      !action.retryTurnId &&
      !action.translateTurnId
        ? { replyToId: this.selectedTurnId }
        : {}),
      ...action,
      ...(base.purpose === 'chat' && webSearch ? { webSearch: true } : {}),
    }
    this.#runId = null
    try {
      const result = await this.#port.start(request, onEvent, {
        signal: abort.signal,
      })
      if (this.#abort !== abort) {
        if (!ended) void this.#port.cancel(result.runId).catch(() => undefined)
        return true
      }
      if (abort.signal.aborted && !ended)
        void this.#port.cancel(result.runId).catch(() => undefined)
      accepted = true
      this.#runId = result.runId
      if (result.conversationId) this.conversationId = result.conversationId
      if (result.turnId) {
        const temporaryId = turn.id
        turn.id = result.turnId
        turn.persisted = true
        if (turn.questionId === temporaryId) turn.questionId = result.turnId
        if (this.selectedTurnId === temporaryId)
          this.selectedTurnId = result.turnId
      }
      void settle()
      return true
    } catch (error: unknown) {
      if (this.#abort !== abort) return true
      const code = isKitClientError(error) ? error.code : 'internal'
      if (isKitClientError(error)) this.#checkSkill(error)
      finish(
        code === 'cancelled' ? 'cancelled' : 'failed',
        code === 'cancelled' ? null : code,
      )
      if (
        (turn.supersedesId !== null || turn.kind === 'translation') &&
        code !== 'cancelled'
      )
        this.actionError = code
      if (turn.supersedesId !== null || turn.kind === 'translation')
        this.turns = this.turns.filter((item) => item !== turn)
      this.#abort = null
      this.#runId = null
      this.running = false
      return code === 'cancelled'
    }
  }

  #checkSkill(error: Pick<KitError, 'code' | 'details'> | null | undefined) {
    if (
      error?.code === 'conflict' &&
      error.details?.reason === 'skill_changed'
    ) {
      this.skillChanged = error.details.entity ?? ''
    }
  }
}
