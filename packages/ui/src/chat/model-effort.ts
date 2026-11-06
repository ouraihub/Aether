// Pure rules for the composer model/effort picker (docs/31 §3–§4.2): search, ranking,
// carry-over of an explicit effort and the secret-free row projection. No I/O.
import type { KitI18n } from '../i18n'
import type { ModelOption, Provider, ReasoningEffort } from '../ports'
import { REASONING_EFFORTS } from '../providers/reasoning'

/** One selectable model as the picker shows it; `key` stays an opaque host identity. */
export interface PickerModel {
  key: string
  providerId: string
  provider: string
  name: string
  /** Route, price and context facts; empty parts are omitted, never guessed. */
  detail: string
  /** The route has an enabled, readable key. Keyless models stay selectable. */
  hasKey: boolean
  /** Declared levels in display order, without `default`. */
  efforts: ReasoningEffort[]
  contextWindow: number | null
}

/** `effort: 'default'` sends no reasoning control; it is not `none`. */
export interface PickerValue {
  key: string
  effort: ReasoningEffort
}

export const modelKey = (endpointId: string, modelId: string) => `${endpointId}\n${modelId}`

/** Splits a trailing level (`opus high`); `off` is accepted for `none`. */
export function parseQuery(query: string): { text: string; effort: ReasoningEffort | null } {
  const trimmed = query.trim()
  const match = /^(.*\S)\s+(\S+)$/.exec(trimmed)
  if (match?.[1] && match[2]) {
    const word = match[2].toLowerCase()
    const effort = word === 'off' ? 'none' : REASONING_EFFORTS.find((item) => item === word)
    if (effort) return { text: match[1].trim(), effort }
  }
  return { text: trimmed, effort: null }
}

/** Lower is better; `null` hides the model (no match, or the typed level is not declared). */
export function score(model: PickerModel, query: string): number | null {
  const { text, effort } = parseQuery(query)
  if (effort && !model.efforts.includes(effort)) return null
  const haystack = `${model.name} ${model.provider} ${model.detail}`.toLowerCase()
  const needle = text.toLowerCase()
  const index = haystack.indexOf(needle)
  if (index >= 0) return index
  const parts = needle.split(/\s+/).filter(Boolean)
  if (parts.every((part) => haystack.includes(part))) return 500
  let rank = 1000
  let from = 0
  for (const char of needle.replace(/\s+/g, '')) {
    const found = haystack.indexOf(char, from)
    if (found < 0) return null
    rank += found
    from = found + 1
  }
  return rank
}

/** Typed level first, then the current explicit level if declared, else `default`. */
export function nextValue(
  model: PickerModel,
  current: PickerValue | null,
  query: string,
): PickerValue {
  const typed = parseQuery(query).effort
  if (typed && model.efforts.includes(typed)) return { key: model.key, effort: typed }
  const kept = current?.effort
  const effort = kept && kept !== 'default' && model.efforts.includes(kept) ? kept : 'default'
  return { key: model.key, effort }
}

export interface PickerGroup {
  id: string
  provider: string
  models: PickerModel[]
}

/** Groups in first-seen order; models inside a group are ranked when searching. */
export function groupModels(models: readonly PickerModel[], query: string): PickerGroup[] {
  const searching = query.trim() !== ''
  const groups: PickerGroup[] = []
  for (const model of models) {
    if (searching && score(model, query) === null) continue
    const group = groups.find((item) => item.id === model.providerId)
    if (group) group.models.push(model)
    else groups.push({ id: model.providerId, provider: model.provider, models: [model] })
  }
  if (searching)
    for (const group of groups)
      group.models.sort((a, b) => (score(a, query) ?? Infinity) - (score(b, query) ?? Infinity))
  return groups
}

/** Header facts: key presence, the first model's route and the model count. */
export function groupSummary(group: PickerGroup, i18n: KitI18n): string {
  const route = group.models[0]?.detail.split(' · ')[0]?.trim() ?? ''
  const keyed = group.models.some((model) => model.hasKey)
  const head = [keyed ? i18n.t('chat.picker.keyed') : '', route].filter(Boolean).join(' · ')
  return `${head} ${group.models.length}`.trim()
}

/** Filled bars (0–4) of the trigger; `none` is not a reasoning rank. */
export function effortBars(model: PickerModel, effort: ReasoningEffort): number {
  if (effort === 'default' || effort === 'none') return 0
  const ranked = model.efforts.filter((item) => item !== 'none')
  const rank = ranked.indexOf(effort)
  if (rank < 0) return 0
  return Math.round(((rank + 1) / ranked.length) * 4)
}

/** Color family of a level: fast (info), deep (warning) or regular (primary). */
export function effortTone(effort: ReasoningEffort): 'fast' | 'deep' | 'regular' {
  if (effort === 'none' || effort === 'minimal' || effort === 'low') return 'fast'
  if (effort === 'xhigh' || effort === 'max') return 'deep'
  return 'regular'
}

function contextSize(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`
  if (tokens >= 1000) return `${Math.floor(tokens / 1000)}K`
  return String(tokens)
}

/**
 * Joins options with provider metadata for the second row. Without `providers` the picker still
 * works; route names fall back to the option label, prices and windows are left out.
 */
export function pickerModels(
  options: readonly ModelOption[],
  providers: readonly Provider[] | null,
  i18n: KitI18n,
): PickerModel[] {
  return options.map((option) => {
    const segments = option.label.split(' · ')
    let provider: Provider | undefined
    let endpoint: Provider['endpoints'][number] | undefined
    for (const item of providers ?? []) {
      endpoint = item.endpoints.find((candidate) => candidate.id === option.endpointId)
      if (endpoint) {
        provider = item
        break
      }
    }
    const model = endpoint?.models.find((candidate) => candidate.id === option.modelId)
    const name = option.name?.trim() || option.modelId
    const route = endpoint?.slug ?? segments[1] ?? ''
    const parts = [route]
    if (name !== option.modelId) parts.push(option.modelId)
    const tier = model?.pricing?.tiers[0]
    if (model?.pricing && tier)
      parts.push(
        i18n.t('chat.picker.price', {
          currency: model.pricing.currency,
          input: tier.input,
          output: tier.output,
        }),
      )
    const contextWindow = model?.contextWindow ?? null
    if (contextWindow)
      parts.push(i18n.t('chat.picker.context', { size: contextSize(contextWindow) }))
    const declared = option.reasoning?.efforts ?? []
    return {
      key: modelKey(option.endpointId, option.modelId),
      providerId: provider?.id ?? segments[0] ?? option.endpointId,
      provider: provider?.name ?? segments[0] ?? option.endpointId,
      name,
      detail: parts.filter(Boolean).join(' · '),
      hasKey: endpoint?.keys.some((key) => key.enabled && key.readable) ?? false,
      efforts: REASONING_EFFORTS.filter((effort) => declared.includes(effort)),
      contextWindow,
    }
  })
}
