// One run: performs the actions of `RunMachine` (model requests, host authorization, approval
// waits, tool execution) and owns usage, context accounting and the single `end`.
import type { ContextUsage, RunEvent, TokenUsage } from '@ouraihub/aether-contracts'
import type { Approver, ArgumentsValidator, EventSink, OutboundHttp, ToolExecutor } from '../ports.js'
import type { Message, RunInput, RunSummary } from '../types.js'
import { KitFailure, safeError } from '../error.js'
import { addUsage, cost, emptyUsage } from '../pricing.js'
import { RunMachine } from './machine.js'
import type { Action } from './machine.js'
import { streamRound } from './model.js'
import type { RoundResult, RoundState } from './model.js'
import { boundaryId, contextUsageForRequest, renderSystem, validate } from './prepare.js'
import { executeCall, waitApproval } from './tools.js'
export {
  validate,
  renderSystem,
  reasoningBudget,
  validateReasoning,
  estimateContextTokens,
  contextUsageForRequest,
  proposedApproval,
} from './prepare.js'
export interface RunEngineOptions {
  outbound: OutboundHttp
  tools: ToolExecutor
  approver: Approver
  validateArguments: ArgumentsValidator
  toolTimeoutMs?: number
  approvalTimeoutMs?: number
}
export class RunEngine {
  private readonly options: RunEngineOptions
  constructor(options: RunEngineOptions) {
    this.options = options
  }
  validate(input: RunInput): RunInput {
    return validate(input)
  }
  async run(raw: RunInput, signal: AbortSignal, onEvent: EventSink): Promise<RunSummary> {
    let messages: Message[] = []
    const rounds: TokenUsage[] = []
    const usage = emptyUsage()
    let contextUsage: ContextUsage = {
      tokens: null,
      contextWindow: raw.connection.contextWindow,
      source: 'unknown',
      endpointId: raw.connection.endpointId,
      modelId: raw.connection.modelId,
    }
    let exhausted = false
    let end: Extract<RunEvent, { kind: 'end' }>
    let sinkFailed = false
    const controller = new AbortController()
    const cancel = (): void => controller.abort()
    if (signal.aborted) controller.abort()
    signal.addEventListener('abort', cancel, { once: true })
    const emit: EventSink = (event) => {
      if (sinkFailed) return
      try {
        onEvent(event)
      } catch {
        sinkFailed = true
        controller.abort()
      }
    }
    const state: RoundState = { text: '', reasoning: [], requestChars: 0 }
    let machine: RunMachine | undefined
    try {
      const input = validate(raw)
      const system = renderSystem(input.system, input.permissionMode === 'plan')
      const run = new RunMachine({
        input,
        system,
        boundary: boundaryId(),
        signal: controller.signal,
        state,
        emit,
        validateArguments: this.options.validateArguments,
      })
      machine = run
      messages = run.messages
      emit({ kind: 'context', usage: contextUsage })
      let action: Action = run.start()
      for (;;) {
        if (action.kind === 'done') {
          exhausted = action.exhausted
          break
        }
        if (action.kind === 'failed') throw action.error
        if (action.kind === 'model') {
          const roundUsage = emptyUsage()
          const stream = (): Promise<RoundResult> =>
            streamRound(this.options.outbound, run.input, system, messages, controller.signal, roundUsage, state, emit)
          let result: RoundResult
          try {
            try {
              result = await stream()
            } catch (error) {
              if (!run.retryWithoutTools(error)) throw error
              result = await stream()
            }
          } finally {
            rounds.push(roundUsage)
            addUsage(usage, roundUsage)
          }
          contextUsage = contextUsageForRequest({
            inputTokens: roundUsage.inputTokens,
            outputTokens: roundUsage.outputTokens,
            estimatedInputTokens: Math.ceil(state.requestChars / 3),
            contextWindow: run.input.connection.contextWindow,
            endpointId: run.input.connection.endpointId,
            modelId: run.input.connection.modelId,
          })
          emit({ kind: 'usage', ...usage, cost: cost(run.input.connection.pricing, rounds) })
          emit({ kind: 'context', usage: contextUsage })
          action = run.modelDone(result)
        } else if (action.kind === 'authorize')
          action = run.authorized(action.pending, this.options.tools.authorize(action.pending.call, action.context))
        else if (action.kind === 'approve') {
          const allow = await waitApproval(
            this.options.approver,
            input.runId,
            action.pending.call,
            controller.signal,
            this.options.approvalTimeoutMs ?? 300000,
          )
          action = run.approved(action.pending, allow)
        } else {
          const execution = await executeCall(
            this.options.tools,
            action.pending.call,
            controller.signal,
            this.options.toolTimeoutMs ?? 120000,
          )
          action = run.executed(action.pending, execution)
        }
      }
      end = { kind: 'end', status: 'completed', ...(exhausted ? { exhausted: true } : {}) }
    } catch (error) {
      machine?.flush()
      contextUsage = { ...contextUsage, tokens: null, source: 'unknown' }
      end =
        controller.signal.aborted || (error instanceof KitFailure && error.code === 'cancelled')
          ? { kind: 'end', status: 'cancelled' }
          : { kind: 'end', status: 'failed', error: safeError(error) }
    } finally {
      signal.removeEventListener('abort', cancel)
    }
    const spent = cost(raw.connection.pricing, rounds)
    emit({ kind: 'usage', ...usage, cost: spent })
    emit({ kind: 'context', usage: contextUsage })
    emit(end)
    return {
      status: end.status,
      messages,
      usage,
      cost: spent,
      contextUsage,
      exhausted,
      ...(end.error ? { error: end.error } : {}),
    }
  }
}
