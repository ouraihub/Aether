// One conversation in the panel: turns, the active run and its abort (docs/16 §4.2, §7 trap 4).
import { isKitClientError } from '../ports'
import type {
  ChatPort,
  ContextRef,
  KitError,
  ModelRef,
  RunEvent,
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

import type { ContextUsage, TokenUsage, ReasoningEffort } from '../ports'

export interface ChatTurn {
  input: string
  text: string
  reasoningText: string
  reasoningEffort: ReasoningEffort
  usage: TokenUsage | null
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
}

export class RunSession {
  turns = $state<ChatTurn[]>([])
  conversationId = $state<string | null>(null)
  running = $state(false)
  contextUsage = $state<ContextUsage | null>(null)
  search = $state<SearchState>({ status: 'idle', query: null, provider: null, hits: [], error: null })
  /** Id of a selected skill the backend rejected as `skill_changed`; the user has to confirm it again. */
  skillChanged = $state<string | null>(null)
  #abort: AbortController | null = null
  /** Id of the active run, known once `chat_run` resolves. */
  #runId: string | null = null
  readonly #port: ChatPort

  constructor(port: ChatPort) {
    this.#port = port
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

  send(base: RunBase, input: string, webSearch = false) {
    const search: SearchState = {
      status: webSearch ? 'searching' : 'idle',
      query: webSearch ? input : null,
      provider: null,
      hits: [],
      error: null,
    }
    if (this.running || input.trim() === '') return Promise.resolve(false)
    this.turns.push({
      input,
      text: '',
      reasoningText: '',
      reasoningEffort: base.reasoningEffort ?? 'default',
      usage: null,
      contextUsage: null,
      status: 'running',
      error: null,
      tools: [],
      exhausted: false,
      search,
    })
    return this.#start(base, input, false, webSearch)
  }

  /** Reruns the last turn in place: stored (`regenerate`) with history, a fresh run without. */
  regenerate(base: RunBase, webSearch = false) {
    const last = this.turns.at(-1)
    if (!last || this.running) return
    Object.assign(last, {
      text: '',
      reasoningText: '',
      reasoningEffort: base.reasoningEffort ?? 'default',
      usage: null,
      contextUsage: null,
      status: 'running',
      error: null,
      tools: [],
      exhausted: false,
    })
    void this.#start(base, last.input, this.conversationId !== null, webSearch)
  }

  /** Cancels the active run; its `end` arrives as `cancelled`. */
  stop() {
    this.#abort?.abort()
  }

  /** Answers an `approval_required` call of the active run. */
  approve(tool: ToolItem, allow: boolean) {
    const runId = this.#runId
    if (tool.pending === null || runId === null) return
    const preview = tool.pending
    tool.pending = null
    // Not delivered: show the card again so the user can retry or stop.
    this.#port.approve(runId, tool.callId, allow).catch(() => {
      if (this.running && tool.outcome === null) tool.pending = preview
    })
  }

  show(conversationId: string | null, turns: readonly Turn[] = []) {
    this.stop()
    this.#abort = null
    this.running = false
    this.contextUsage = turns.at(-1)?.contextUsage ?? null
    this.conversationId = conversationId
    this.search = { status: 'idle', query: null, provider: null, hits: [], error: null }
    this.turns = turns.map((t) => ({
      input: t.input,
      text: t.text,
      reasoningText: t.reasoningText ?? '',
      reasoningEffort: t.reasoningEffort ?? 'default',
      usage: t.usage ?? null,
      contextUsage: t.contextUsage ?? null,
      status: t.status,
      error: t.errorCode,
      tools: [],
      exhausted: false,
      search: { status: 'idle', query: null, provider: null, hits: [], error: null },
    }))
  }

  #start(base: RunBase, input: string, regenerate: boolean, webSearch: boolean) {
    // The proxied element, so streamed writes are reactive.
    const turn = this.turns[this.turns.length - 1]
    if (!turn) return Promise.resolve(false)
    const abort = new AbortController()
    this.#abort = abort
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
    const finish = (status: TurnStatus, error: string | null) => {
      if (this.#abort !== abort) return
      turn.status = status
      turn.error = error
      if (status !== 'completed') {
        turn.contextUsage = null
        this.contextUsage = null
      }
      if (this.search.status === 'searching')
        this.search = { ...this.search, status: status === 'failed' ? 'failed' : 'idle', error }
      this.#abort = null
      this.running = false
    }
    const onEvent = (event: RunEvent) => {
      if (this.#abort !== abort) return
      const tool = (callId: string) => turn.tools.find((x) => x.callId === callId)
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
          if (item) Object.assign(item, { pending: null, outcome: event.outcome, summary: event.summary })
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
    const request = {
      purpose: base.purpose,
      model: base.model,
      skills: base.skills,
      context: base.context,
      input: regenerate ? '' : input,
      conversationId: this.conversationId,
      scope: base.scope,
      regenerate,
      language: base.language,
      reasoningEffort: base.reasoningEffort ?? 'default',
      ...(base.purpose === 'chat' && webSearch ? { webSearch: true } : {}),
    }
    this.#runId = null
    return this.#port.run(request, onEvent, { signal: abort.signal }).then(
      (accepted) => {
        if (this.#abort !== abort && this.#abort !== null) return true
        if (!this.turns.includes(turn)) return true
        this.#runId = accepted.runId
        if (accepted.conversationId) this.conversationId = accepted.conversationId
        return true
      },
      // Rejected before acceptance: no `end` will follow.
      (error: unknown) => {
        if (this.#abort !== abort) return true
        const code = isKitClientError(error) ? error.code : 'internal'
        if (isKitClientError(error)) this.#checkSkill(error)
        finish(code === 'cancelled' ? 'cancelled' : 'failed', code === 'cancelled' ? null : code)
        return code === 'cancelled'
      },
    )
  }

  #checkSkill(error: Pick<KitError, 'code' | 'details'> | null | undefined) {
    if (error?.code === 'conflict' && error.details?.reason === 'skill_changed') {
      this.skillChanged = error.details.entity ?? ''
    }
  }
}
