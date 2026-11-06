// Pure rules for the inline context status beside Send (docs/31 §4.3). No I/O.
import type { ContextUsage, Cost, TokenUsage } from '../ports'

export type Pressure = 'normal' | 'warning' | 'high' | 'critical'
export type Platform = 'windows' | 'macos' | 'linux' | 'other'

/** Occupancy in percent, clamped to 0–100; `null` when tokens or window are unknown. */
export function contextPercent(context: ContextUsage | null, window: number | null): number | null {
  const tokens = context?.tokens
  if (tokens == null || window == null || window <= 0) return null
  return Math.min(100, Math.max(0, (tokens / window) * 100))
}

/** Highest of percentage and absolute pressure; absolute counts need a known window. */
export function pressure(context: ContextUsage | null, window: number | null): Pressure {
  const percent = contextPercent(context, window) ?? 0
  const tokens = window != null && window > 0 ? (context?.tokens ?? 0) : 0
  if (percent >= 90 || tokens >= 500_000) return 'critical'
  if (percent >= 70 || tokens >= 270_000) return 'high'
  if (percent >= 50 || tokens >= 150_000) return 'warning'
  return 'normal'
}

/** Truncates rather than rounds, so 999 999 never reads as 1000k. */
export function compact(value: number): string {
  const n = Math.max(0, Math.floor(value))
  const [divisor, suffix] =
    n >= 1e9 ? [1e9, 'B'] : n >= 1e6 ? [1e6, 'M'] : n >= 1e3 ? [1e3, 'k'] : [1, '']
  if (divisor === 1) return String(n)
  const tenth = Math.floor((n * 10) / divisor)
  const whole = Math.floor(tenth / 10)
  return tenth % 10 === 0 ? `${whole}${suffix}` : `${whole}.${tenth % 10}${suffix}`
}

/** `~12.3%/1.1M`; unknown parts render as `?`. */
export function summary(context: ContextUsage | null, window: number | null): string {
  const percent = contextPercent(context, window)
  const size = window != null && window > 0 ? compact(window) : '?'
  const estimated = context?.source === 'estimated' ? '~' : ''
  return `${estimated}${percent === null ? '?' : percent.toFixed(1)}%/${size}`
}

/** Estimated money from configured prices; invalid amounts are unknown. */
export function money({ amount, currency }: Cost): string | null {
  if (!Number.isFinite(amount) || amount < 0) return null
  const text =
    amount > 0 && amount < 0.0001
      ? '<0.0001'
      : amount > 0 && amount < 0.01
        ? amount.toFixed(4)
        : amount.toFixed(2)
  return `≈${currency === 'CNY' ? '¥' : '$'}${text}`
}

export interface SpendingTurn {
  usage: TokenUsage | null
  cost: Cost | null
}

/**
 * Money only when every turn with usage has a priced cost in one currency; otherwise tokens.
 * Input already contains cache, output contains reasoning, so only the two totals are added.
 */
export function spending(turns: readonly SpendingTurn[]): string {
  const used = turns.filter(
    ({ usage }) => usage != null && (usage.inputTokens != null || usage.outputTokens != null),
  )
  if (used.length === 0) return '? tokens'
  const priced = used.flatMap(({ usage, cost }) =>
    cost &&
    Number.isFinite(cost.amount) &&
    cost.amount >= 0 &&
    usage?.inputTokens != null &&
    usage.outputTokens != null
      ? [cost]
      : [],
  )
  const first = priced[0]
  const oneCurrency = priced.every((cost) => cost.currency === first?.currency)
  if (first && priced.length === used.length && oneCurrency) {
    const amount = priced.reduce((sum, cost) => sum + cost.amount, 0)
    const text = money({ amount, currency: first.currency })
    if (text) return text
  }
  let tokens = 0
  for (const { usage } of used) tokens += (usage?.inputTokens ?? 0) + (usage?.outputTokens ?? 0)
  return `${compact(tokens)} tokens`
}

type PlatformNavigator = Navigator & { userAgentData?: { platform?: string } }

/** Browser-reported OS; on the web this is the visitor's system, not the server's. */
export function detectPlatform(
  nav: PlatformNavigator | undefined = globalThis.navigator,
): Platform {
  const name = (nav?.userAgentData?.platform || nav?.platform || '').toLowerCase()
  if (name.startsWith('win')) return 'windows'
  if (name.startsWith('mac') || name.includes('darwin')) return 'macos'
  if (name.includes('linux') && !name.includes('android')) return 'linux'
  return 'other'
}
