import type {
  Protocol,
  ReasoningEffort,
  ReasoningDialect,
  ReasoningConfig,
} from '../ports'

export const REASONING_EFFORTS: ReasoningEffort[] = [
  'none',
  'auto',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
]
export const REASONING_DIALECTS: ReasoningDialect[] = [
  'openai',
  'anthropic-budget',
  'anthropic-adaptive',
  'thinking-type',
  'qwen',
]

/** Whether `dialect` is written on `protocol` (same rule as the backends). */
export function dialectFits(
  dialect: ReasoningDialect,
  protocol: Protocol,
): boolean {
  if (dialect === 'anthropic-budget' || dialect === 'anthropic-adaptive')
    return protocol === 'anthropic-messages'
  if (dialect === 'openai') return protocol !== 'anthropic-messages'
  return protocol === 'openai-chat'
}

/** `auto` (on, depth left to the provider) exists only for switch-style dialects. */
export const allowsAuto = (dialect: ReasoningDialect): boolean =>
  dialect === 'thinking-type' || dialect === 'qwen'

export function emptyReasoning(
  dialect: ReasoningDialect = 'openai',
): ReasoningConfig {
  return {
    dialect,
    efforts: allowsAuto(dialect) ? ['none', 'auto'] : ['low', 'medium', 'high'],
    ...(dialect === 'anthropic-budget' ? { budgetTokens: 1024 } : {}),
  }
}

/** The saved choice if allowed, else the nearest allowed effort (ties go higher); `default` without one. */
export function nearestEffort(
  chosen: ReasoningEffort,
  allowed: readonly ReasoningEffort[],
): ReasoningEffort {
  if (chosen === 'default' || allowed.includes(chosen))
    return allowed.length ? chosen : 'default'
  const at = REASONING_EFFORTS.indexOf(chosen)
  let best: ReasoningEffort = 'default'
  let bestDistance = Infinity
  for (const effort of allowed) {
    const distance = Math.abs(REASONING_EFFORTS.indexOf(effort) - at)
    if (distance <= bestDistance) [best, bestDistance] = [effort, distance]
  }
  return best
}
