import type { ContextUsage, RunEvent, TokenUsage } from '@ouraihub/aether-contracts'
import type { Approver, ArgumentsValidator, EventSink, OutboundHttp, ToolExecutor } from '../ports.js'
import type { Message, RunInput, RunSummary, ToolCall } from '../types.js'
import { cancelled, KitFailure, safeError } from '../error.js'
import { canonicalJson, charCount } from '../guard.js'
import { messageChars } from '../history.js'
import { addUsage, cost, emptyUsage } from '../pricing.js'
import { streamRound } from './model.js'
import type { RoundResult, RoundState } from './model.js'
import { renderSystem, validate } from './prepare.js'
import { executeTool } from './tools.js'
import type { ToolResult } from './tools.js'
export { validate, renderSystem, reasoningBudget, proposedApproval } from './prepare.js'
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
    const messages: Message[] = [],
      rounds: TokenUsage[] = []
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
    const flush = (calls: ToolCall[] = []): void => {
      if (state.text || state.reasoning.length || calls.length)
        messages.push({
          assistant: {
            text: state.text,
            tool_calls: calls,
            ...(state.reasoning.length ? { reasoning: state.reasoning } : {}),
          },
        })
      state.text = ''
      state.reasoning = []
    }
    try {
      let input = validate(raw)
      const system = renderSystem(input.system, input.permissionMode === 'plan')
      const seen = new Map<string, { name: string; args: string; result: ToolResult }>()
      emit({ kind: 'context', usage: contextUsage })
      for (let round = 1; round <= input.budget.maxRounds; round++) {
        if (controller.signal.aborted) throw cancelled()
        const currentChars =
          charCount(system + input.input) +
          charCount(
            JSON.stringify(input.tools.map(({ name, description, parameters }) => ({ name, description, parameters }))),
          ) +
          input.history.reduce((sum, message) => sum + messageChars(message), 0) +
          messages.reduce((sum, message) => sum + messageChars(message), 0)
        if (Math.ceil(currentChars / 3) + input.budget.maxOutput > input.budget.contextWindow)
          throw new KitFailure('budget_exceeded', 'Run exceeds context budget', {
            reason: 'context',
            limit: input.budget.contextWindow,
          })
        emit({ kind: 'round_start', round })
        const roundUsage = emptyUsage()
        let result: RoundResult
        try {
          try {
            result = await streamRound(
              this.options.outbound,
              input,
              system,
              messages,
              controller.signal,
              roundUsage,
              state,
              emit,
            )
          } catch (error) {
            if (
              !(error instanceof KitFailure && error.details?.reason === 'tools_unsupported') ||
              !input.tools.length ||
              input.connection.supportsTools !== null ||
              state.text ||
              state.reasoning.length
            )
              throw error
            input = { ...input, tools: [] }
            result = await streamRound(
              this.options.outbound,
              input,
              system,
              messages,
              controller.signal,
              roundUsage,
              state,
              emit,
            )
          }
        } finally {
          rounds.push(roundUsage)
          addUsage(usage, roundUsage)
        }
        const reported = roundUsage.inputTokens !== null && roundUsage.outputTokens !== null
        contextUsage = {
          ...contextUsage,
          source: reported ? 'reported' : 'estimated',
          tokens: reported
            ? Math.min(0xffff_ffff, (roundUsage.inputTokens ?? 0) + (roundUsage.outputTokens ?? 0))
            : Math.ceil(state.requestChars / 3),
        }
        emit({ kind: 'usage', ...usage, cost: cost(input.connection.pricing, rounds) })
        emit({ kind: 'context', usage: contextUsage })
        if (result.truncated) {
          flush()
          messages.push({ user: '上一条回复被截断，工具参数不完整，已全部作废，请重新发起、参数写短一些' })
        } else if (!result.calls.length) {
          flush()
          exhausted = false
          break
        } else {
          flush(result.calls)
          for (const call of result.calls) {
            if (controller.signal.aborted) throw cancelled()
            const args = canonicalJson(call.args),
              previous = seen.get(call.id)
            if (previous && (previous.name !== call.name || previous.args !== args))
              throw new KitFailure('conflict', 'Tool call ID reused with different arguments', {
                reason: 'tool_call_id_conflict',
              })
            const result =
              previous?.result ??
              (await executeTool(
                input,
                call,
                this.options.tools,
                this.options.approver,
                this.options.validateArguments,
                controller.signal,
                emit,
                this.options.toolTimeoutMs ?? 120000,
                this.options.approvalTimeoutMs ?? 300000,
              ))
            if (!previous) seen.set(call.id, { name: call.name, args, result })
            else
              emit({
                kind: 'tool_result',
                callId: call.id,
                outcome: result.outcome,
                summary: result.text,
                ...(result.payload === undefined ? {} : { payload: result.payload }),
              })
            messages.push({ tool: { call_id: call.id, text: result.text } })
            if (controller.signal.aborted) throw cancelled()
          }
        }
        if (round === input.budget.maxRounds) exhausted = true
      }
      if (controller.signal.aborted) throw cancelled()
      if (exhausted) {
        const text = '已达到本次运行的轮数上限。请继续提问以进行下一步。'
        emit({ kind: 'text_delta', text })
        messages.push({ assistant: { text, tool_calls: [] } })
      }
      end = { kind: 'end', status: 'completed', ...(exhausted ? { exhausted: true } : {}) }
    } catch (error) {
      flush()
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
