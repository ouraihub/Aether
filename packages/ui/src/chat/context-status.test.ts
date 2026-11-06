import { describe, expect, it } from 'vitest'
import type { ContextUsage, TokenUsage } from '../ports'
import { compact, detectPlatform, money, pressure, spending, summary } from './context-status'

const ctx = (tokens: number | null, source: ContextUsage['source'] = 'reported') =>
  ({ tokens, contextWindow: null, source, endpointId: null, modelId: null }) as ContextUsage
const usage = (input: number | null, output: number | null): TokenUsage => ({
  inputTokens: input,
  outputTokens: output,
  cacheReadTokens: null,
  cacheWriteTokens: null,
  reasoningTokens: null,
})

describe('context status', () => {
  it('uses the highest percentage or absolute pressure, absolute only with a window', () => {
    const cases: [number, number, string][] = [
      [499, 1000, 'normal'],
      [500, 1000, 'warning'],
      [699, 1000, 'warning'],
      [700, 1000, 'high'],
      [899, 1000, 'high'],
      [900, 1000, 'critical'],
      [150_000, 10_000_000, 'warning'],
      [270_000, 10_000_000, 'high'],
      [500_000, 10_000_000, 'critical'],
    ]
    for (const [tokens, window, expected] of cases)
      expect(pressure(ctx(tokens), window), `${tokens}/${window}`).toBe(expected)
    expect(pressure(ctx(900_000), null)).toBe('normal')
  })

  it('formats the summary with unknown parts and the estimate marker', () => {
    expect(summary(ctx(1000, 'estimated'), 1_000_000)).toBe('~0.1%/1M')
    expect(summary(ctx(null, 'unknown'), 1_100_000)).toBe('?%/1.1M')
    expect(summary(ctx(10), null)).toBe('?%/?')
    expect(summary(ctx(5000), 1000)).toBe('100.0%/1k')
  })

  it('truncates compact counts instead of rounding up across a unit', () => {
    expect(compact(999)).toBe('999')
    expect(compact(1050)).toBe('1k')
    expect(compact(999_999)).toBe('999.9k')
    expect(compact(1_100_000)).toBe('1.1M')
  })

  it('formats estimated money and rejects invalid amounts', () => {
    expect(money({ amount: 1.5159, currency: 'USD' })).toBe('≈$1.52')
    expect(money({ amount: 0.005, currency: 'CNY' })).toBe('≈¥0.0050')
    expect(money({ amount: 0.00001, currency: 'USD' })).toBe('≈$<0.0001')
    expect(money({ amount: 0, currency: 'USD' })).toBe('≈$0.00')
    expect(money({ amount: Number.NaN, currency: 'USD' })).toBeNull()
    expect(money({ amount: -1, currency: 'CNY' })).toBeNull()
  })

  it('shows money only when every used turn is priced in one currency', () => {
    const priced = { usage: usage(1000, 50), cost: { amount: 1.5, currency: 'USD' as const } }
    const unpriced = { usage: usage(1000, 50), cost: null }
    expect(spending([])).toBe('? tokens')
    expect(spending([{ usage: null, cost: null }])).toBe('? tokens')
    expect(spending([priced, priced])).toBe('≈$3.00')
    expect(spending([priced, unpriced])).toBe('2.1k tokens')
    const yuan = { usage: usage(1, 1), cost: { amount: 1, currency: 'CNY' as const } }
    expect(spending([priced, yuan])).toBe('1k tokens')
    expect(spending([{ usage: null, cost: null }, priced])).toBe('≈$1.50')
    const invalid = { usage: usage(1000, 50), cost: { amount: -1, currency: 'USD' as const } }
    expect(spending([priced, invalid])).toBe('2.1k tokens')
    const partial = { usage: usage(1000, null), cost: { amount: 1, currency: 'USD' as const } }
    expect(spending([priced, partial])).toBe('2k tokens')
  })

  it('maps browser platform names to the shown system mark', () => {
    const nav = (platform: string, hint?: string) =>
      ({ platform, userAgentData: hint ? { platform: hint } : undefined }) as unknown as Navigator
    expect(detectPlatform(nav('Win32'))).toBe('windows')
    expect(detectPlatform(nav('MacIntel'))).toBe('macos')
    expect(detectPlatform(nav('Linux x86_64'))).toBe('linux')
    expect(detectPlatform(nav('Linux armv8l', 'Android'))).toBe('other')
    expect(detectPlatform(nav('', 'Windows'))).toBe('windows')
    expect(detectPlatform(undefined)).toBe('other')
  })
})
