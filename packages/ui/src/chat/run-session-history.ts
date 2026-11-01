import type { Cost, TokenUsage, Turn } from '../ports'
import type { ChatTurn } from './run-session-types'

export function fromHistory(turn: Turn): ChatTurn {
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
    search: { status: 'idle', query: null, provider: null, hits: [], error: null },
  }
}

export function cumulativeUsage(turns: readonly ChatTurn[]): TokenUsage {
  const total: TokenUsage = {
    inputTokens: null,
    outputTokens: null,
    cacheReadTokens: null,
    cacheWriteTokens: null,
    reasoningTokens: null,
  }
  for (const turn of turns)
    for (const key of Object.keys(total) as (keyof TokenUsage)[]) {
      const value = turn.usage?.[key]
      if (value != null) total[key] = (total[key] ?? 0) + value
    }
  return total
}

export function cumulativeCost(turns: readonly ChatTurn[]): Cost[] {
  const totals: Cost[] = []
  for (const { cost } of turns) {
    if (!cost) continue
    const found = totals.find((item) => item.currency === cost.currency)
    if (found) found.amount += cost.amount
    else totals.push({ ...cost })
  }
  return totals
}
