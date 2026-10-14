import type { ReasoningEffort, ReasoningDialect, ReasoningConfig } from '../ports'

export const REASONING_EFFORTS: ReasoningEffort[] = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
export const REASONING_DIALECTS: ReasoningDialect[] = ['openai', 'anthropic-budget', 'anthropic-adaptive']

export function emptyReasoning(dialect: ReasoningDialect = 'openai'): ReasoningConfig {
  return {
    dialect,
    efforts: ['low', 'medium', 'high'],
    ...(dialect === 'anthropic-budget' ? { budgetTokens: 1024 } : {}),
  }
}
