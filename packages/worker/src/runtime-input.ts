import type { EndStatus, KitError } from '@ouraihub/aether-contracts'
import { safeError } from '@ouraihub/aether-core/error'
import { charCount } from '@ouraihub/aether-core/guard'
import { renderSystem, validate } from '@ouraihub/aether-core/engine'
import { trimHistory } from '@ouraihub/aether-core/history'
import type { HistoryTurn, RunInput, RunSummary } from '@ouraihub/aether-core/types'

export const emptySummary = (status: EndStatus, error?: KitError): RunSummary => ({
  status,
  messages: [],
  usage: {
    inputTokens: null,
    outputTokens: null,
    cacheReadTokens: null,
    cacheWriteTokens: null,
    reasoningTokens: null,
  },
  cost: null,
  contextUsage: null,
  exhausted: false,
  ...(error ? { error } : {}),
})

/** Final snapshots never copy arbitrary host/upstream exception text or details. */
export const publicError = (error: unknown): KitError => ({
  code: safeError(error).code,
  message: 'Run could not be completed',
})

export function assemble(input: RunInput, history: readonly HistoryTurn[]): RunInput {
  const checked = validate({ ...input, history: [] })
  const systemChars = charCount(renderSystem(checked.system, checked.permissionMode === 'plan'))
  const available =
    checked.budget.contextWindow -
    checked.budget.maxOutput -
    Math.ceil((systemChars + charCount(checked.input)) / 3)
  return {
    ...checked,
    history:
      checked.purpose === 'writing'
        ? []
        : trimHistory(history, available, checked.connection.canVision),
  }
}
