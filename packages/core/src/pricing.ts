import type { Cost, ModelPricing, TokenUsage } from '@ouraihub/aether-contracts'
export const emptyUsage = (): TokenUsage => ({
  inputTokens: null,
  outputTokens: null,
  cacheReadTokens: null,
  cacheWriteTokens: null,
  reasoningTokens: null,
})
const USAGE_KEYS = ['inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens'] as const
export function addUsage(total: TokenUsage, next: TokenUsage): void {
  for (const key of USAGE_KEYS) {
    const left = total[key],
      right = next[key]
    total[key] = left === null ? right : right === null ? left : Math.min(0xffff_ffff, left + right)
  }
}
export function cost(pricing: ModelPricing | null, rounds: readonly TokenUsage[]): Cost | null {
  if (!pricing || !rounds.length) return null
  let amount = 0
  for (const round of rounds) {
    const inputTokens = round.inputTokens
    if (inputTokens === null || round.outputTokens === null) return null
    const tier = pricing.tiers.findLast((candidate) => candidate.minInputTokens <= inputTokens) ?? pricing.tiers[0]
    if (!tier) return null
    const read = round.cacheReadTokens ?? 0,
      write = round.cacheWriteTokens ?? 0
    amount +=
      (Math.max(0, inputTokens - read - write) * tier.input +
        read * (tier.cacheRead ?? tier.input) +
        write * (tier.cacheWrite ?? tier.input) +
        round.outputTokens * tier.output) /
      1e6
  }
  return { amount, currency: pricing.currency }
}
