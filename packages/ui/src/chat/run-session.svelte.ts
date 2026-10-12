// One conversation in the panel: turns, the active run and its abort (docs/16 §4.2, §7 trap 4).
import { isKitClientError } from '../ports'
import type {
  ChatPort,
  ContextRef,
  KitError,
  ModelRef,
  RunEvent,
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

export interface ChatTurn {
  input: string
  text: string
  status: TurnStatus
  /** Error code of a failed turn (`interrupted` after a crash). */
  error: string | null
  /** Tool calls of the live run; stored turns only keep their text. */
  tools: ToolItem[]
  /** The round limit stopped the tool loop. */
  exhausted: boolean
}

export interface RunBase {
  purpose: string
  scope: string | null
  language: string
  model: ModelRef | null
  skills: SkillRef[]
  context: ContextRef | null
}

export class RunSession {
  turns = $state<ChatTurn[]>([])
  conversationId = $state<string | null>(null)
  running = $state(false)
  /** Id of a selected skill the backend rejected as `skill_changed`; the user has to confirm it again. */
  skillChanged = $state<string | null>(null)
  #abort: AbortController | null = null
  /** Id of the active run, known once `chat_run` resolves. */
  #runId: string | null = null
  readonly #port: ChatPort

  constructor(port: ChatPort) {
    this.#port = port
  }

  send(base: RunBase, input: string) {
    this.turns.push({ input, text: '', status: 'running', error: null, tools: [], exhausted: false })
    this.#start(base, input, false)
  }

  /** Reruns the last turn in place: stored (`regenerate`) with history, a fresh run without. */
  regenerate(base: RunBase) {
    const last = this.turns.at(-1)
    if (!last || this.running) return
    Object.assign(last, { text: '', status: 'running', error: null, tools: [], exhausted: false })
    this.#start(base, last.input, this.conversationId !== null)
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
    this.conversationId = conversationId
    this.turns = turns.map((t) => ({
      input: t.input,
      text: t.text,
      status: t.status,
      error: t.errorCode,
      tools: [],
      exhausted: false,
    }))
  }

  #start(base: RunBase, input: string, regenerate: boolean) {
    // The proxied element, so streamed writes are reactive.
    const turn = this.turns[this.turns.length - 1]
    if (!turn) return
    const abort = new AbortController()
    this.#abort = abort
    this.running = true
    const finish = (status: TurnStatus, error: string | null) => {
      turn.status = status
      turn.error = error
      if (this.#abort !== abort) return
      this.#abort = null
      this.running = false
    }
    const onEvent = (event: RunEvent) => {
      const tool = (callId: string) => turn.tools.find((x) => x.callId === callId)
      switch (event.kind) {
        case 'text_delta':
          turn.text += event.text
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
        case 'usage':
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
    }
    this.#runId = null
    this.#port.run(request, onEvent, { signal: abort.signal }).then(
      (accepted) => {
        this.#runId = accepted.runId
        if (accepted.conversationId) this.conversationId = accepted.conversationId
      },
      // Rejected before acceptance: no `end` will follow.
      (error: unknown) => {
        const code = isKitClientError(error) ? error.code : 'internal'
        if (isKitClientError(error)) this.#checkSkill(error)
        finish(code === 'cancelled' ? 'cancelled' : 'failed', code === 'cancelled' ? null : code)
      },
    )
  }

  #checkSkill(error: Pick<KitError, 'code' | 'details'> | null | undefined) {
    if (error?.code === 'conflict' && error.details?.reason === 'skill_changed') {
      this.skillChanged = error.details.entity ?? ''
    }
  }
}
