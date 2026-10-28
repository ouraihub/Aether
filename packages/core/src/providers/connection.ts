import {
  isContract,
  type Model,
  type ModelPricing,
  type Protocol,
  type ReasoningConfig,
} from '@ouraihub/aether-contracts'
import type { Connection } from '../types.js'
import { badRequest, KitFailure } from '../error.js'

export interface EndpointSnapshot {
  endpointId: string | null
  baseUrl: string
  protocol: Protocol
  keys: readonly string[]
  headers: Readonly<Record<string, string>>
  imageBaseUrl?: string | null
  imageEditBaseUrl?: string | null
}
export interface ConnectionSnapshot {
  endpoint: EndpointSnapshot
  model: Model | null
}
export const RESERVED_HEADERS: Readonly<Record<string, true>> = {
  authorization: true,
  'x-api-key': true,
  'anthropic-version': true,
  host: true,
  'content-type': true,
  'content-length': true,
  accept: true,
  connection: true,
  'transfer-encoding': true,
  cookie: true,
}
export const charCount = (value: string): number => Array.from(value).length
export const apiBase = (value: string): string => {
  const base = value.trim().replace(/\/+$/, '')
  return base.endsWith('/v1') ? base : `${base}/v1`
}
export function validateUrl(field: string, raw: string): void {
  const value = raw.trim()
  if (!value) return
  let url: URL
  try {
    url = new URL(value)
  } catch {
    throw badRequest(field, 'invalid_url')
  }
  if (
    new TextEncoder().encode(value).length > 2048 ||
    !['http:', 'https:'].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.hash
  )
    throw badRequest(field, 'invalid_url')
}
export function validateHeaders(input: Readonly<Record<string, string | undefined>>): Record<string, string> {
  const entries = Object.entries(input)
  if (entries.length > 20) throw badRequest('headers', 'too_many')
  const headers: Record<string, string> = {}
  for (const [raw, value] of entries) {
    const name = raw.trim().toLowerCase()
    if (
      !/^[!#$%&'*+.^_`|~0-9a-z-]{1,100}$/.test(name) ||
      Object.hasOwn(headers, name) ||
      typeof value !== 'string' ||
      charCount(value) > 2048 ||
      /[\r\n]/.test(value)
    )
      throw badRequest('headers', 'invalid')
    if (Object.hasOwn(RESERVED_HEADERS, name)) throw badRequest('headers', 'reserved')
    headers[name] = value
  }
  return headers
}
export function authHeaders(
  protocol: Protocol,
  key: string | null,
  custom: Readonly<Record<string, string>> = {},
): Headers {
  const result = new Headers()
  for (const [name, value] of Object.entries(custom))
    if (!Object.hasOwn(RESERVED_HEADERS, name.toLowerCase())) result.set(name, value)
  result.set('accept', 'application/json')
  if (protocol === 'anthropic-messages') {
    result.set('anthropic-version', '2023-06-01')
    if (key) {
      result.set('x-api-key', key)
      result.set('authorization', `Bearer ${key}`)
    }
  } else if (key) result.set('authorization', `Bearer ${key}`)
  return result
}
export function validateReasoning(
  config: ReasoningConfig,
  protocol: Protocol,
  modelType: string,
  maxOutput: number | null,
): void {
  const switchStyle = config.dialect === 'thinking-type' || config.dialect === 'qwen'
  if (
    modelType !== 'chat' ||
    !config.efforts.length ||
    config.efforts.includes('default') ||
    (config.efforts.includes('auto') && !switchStyle) ||
    new Set(config.efforts).size !== config.efforts.length
  )
    throw badRequest('reasoning', 'invalid_efforts')
  const fits =
    config.dialect === 'anthropic-budget' || config.dialect === 'anthropic-adaptive'
      ? protocol === 'anthropic-messages'
      : config.dialect === 'openai'
        ? protocol !== 'anthropic-messages'
        : protocol === 'openai-chat'
  if (!fits) throw badRequest('reasoning', 'protocol_mismatch')
  const budget = config.budgetTokens
  if (config.dialect === 'anthropic-budget') {
    if (budget == null || !Number.isSafeInteger(budget) || budget < 1024 || budget >= (maxOutput ?? 4096))
      throw badRequest('reasoning', 'invalid_budget')
  } else if (config.dialect === 'qwen') {
    if (budget != null && (!Number.isSafeInteger(budget) || budget < 1)) throw badRequest('reasoning', 'invalid_budget')
  } else if (budget != null) throw badRequest('reasoning', 'unexpected_budget')
}
export function validatePricing(pricing: ModelPricing | null, modelType: string): void {
  if (!pricing) return
  const valid = (value: number | null): boolean =>
    value === null || (Number.isFinite(value) && value >= 0 && value <= 1e6)
  if (
    modelType !== 'chat' ||
    !pricing.tiers.length ||
    pricing.tiers.length > 8 ||
    pricing.tiers[0]?.minInputTokens !== 0 ||
    pricing.tiers.some(
      (tier, index) =>
        !Number.isSafeInteger(tier.minInputTokens) ||
        (index > 0 && tier.minInputTokens <= (pricing.tiers[index - 1]?.minInputTokens ?? 0)) ||
        ![tier.input, tier.output, tier.cacheRead, tier.cacheWrite].every(valid),
    )
  )
    throw badRequest('pricing', 'invalid')
}
export function validateModel(model: Model, protocol: Protocol): void {
  if (!isContract('Model', model)) throw badRequest('model', 'invalid')
  if (!model.id.trim()) throw badRequest('models', 'empty_id')
  for (const [field, value] of [
    ['contextWindow', model.contextWindow],
    ['maxOutput', model.maxOutput],
  ] as const)
    if (value !== null && (!Number.isSafeInteger(value) || value < 1 || value > 0xffffffff))
      throw badRequest(field, 'invalid')
  if (
    model.modelType === 'chat' &&
    model.contextWindow !== null &&
    model.maxOutput !== null &&
    model.maxOutput >= model.contextWindow
  )
    throw badRequest('maxOutput', 'exceeds_context')
  for (const [field, value] of [
    ['name', model.name],
    ['groupName', model.groupName],
  ] as const)
    if (value !== null && charCount(value.trim()) > 80) throw badRequest(field, 'too_long', 80)
  if (model.nativeWebSearch && (model.modelType !== 'chat' || protocol === 'openai-chat'))
    throw badRequest('nativeWebSearch', 'unsupported_protocol')
  if (model.reasoning) validateReasoning(model.reasoning, protocol, model.modelType, model.maxOutput)
  validatePricing(model.pricing, model.modelType)
}
/** Prepares one immutable-by-ownership snapshot; network consumers still enforce address policy. */
export function prepare(snapshot: ConnectionSnapshot): Connection {
  const { endpoint, model } = snapshot
  if (!['anthropic-messages', 'openai-chat', 'openai-responses'].includes(endpoint.protocol))
    throw badRequest('protocol', 'invalid')
  validateUrl('baseUrl', endpoint.baseUrl)
  for (const [field, value] of [
    ['imageBaseUrl', endpoint.imageBaseUrl],
    ['imageEditBaseUrl', endpoint.imageEditBaseUrl],
  ] as const)
    if (value != null) validateUrl(field, value)
  const headers = validateHeaders(endpoint.headers)
  if (endpoint.keys.length > 16) throw badRequest('keys', 'too_many')
  for (const key of endpoint.keys)
    if (
      !key.trim() ||
      new TextEncoder().encode(key).length > 4096 ||
      [...key].some((character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127)
    )
      throw badRequest('apiKey', 'invalid')
  if (model) {
    if (!endpoint.baseUrl.trim())
      throw new KitFailure('needs_config', 'Missing endpoint URL', { reason: 'missing_url' })
    validateModel(model, endpoint.protocol)
  }
  return {
    endpointId: endpoint.endpointId,
    modelId: model?.id ?? '',
    protocol: endpoint.protocol,
    baseUrl: apiBase(endpoint.baseUrl),
    keys: [...endpoint.keys],
    headers,
    supportsTools: model?.supportsTools ?? null,
    canVision: model?.canVision ?? false,
    contextWindow: model?.contextWindow ?? null,
    maxOutput: model?.maxOutput ?? null,
    reasoning: model?.reasoning ?? null,
    pricing: model?.pricing ?? null,
    nativeWebSearch: model?.nativeWebSearch ?? false,
    ...(endpoint.imageBaseUrl?.trim() ? { imageBaseUrl: apiBase(endpoint.imageBaseUrl) } : {}),
    ...(endpoint.imageEditBaseUrl?.trim() ? { imageEditBaseUrl: apiBase(endpoint.imageEditBaseUrl) } : {}),
  }
}
