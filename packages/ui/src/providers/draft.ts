// Editable form of a provider and its conversion to save/probe requests (docs/11 §3, §7); key helpers (docs/34 §5).
import type { KitI18n } from '../i18n'
import {
  isKitClientError,
  type Endpoint,
  type EndpointKey,
  type EndpointSave,
  type Model,
  type ProbeTarget,
  type Protocol,
  type Provider,
  type ProviderSave,
} from '../ports'

export interface EndpointDraft {
  /** Stored endpoint, `null` for a new one. Its `id` must go back on save (docs/11 陷阱 1). */
  saved: Endpoint | null
  slug: string
  baseUrl: string
  protocol: Protocol
  /** `null` = paint at `baseUrl` (docs/36 §3.4). */
  imageBaseUrl: string | null
  /** `null` = edit at `baseUrl` (docs/37 §2). */
  imageEditBaseUrl: string | null
  headers: Endpoint['headers']
  models: Model[]
}

export interface ProviderDraft {
  id: string | null
  name: string
  enabled: boolean
  notes: string
  endpoints: EndpointDraft[]
}

export function toDraft(provider: Provider): ProviderDraft {
  return {
    id: provider.id,
    name: provider.name,
    enabled: provider.enabled,
    notes: provider.notes,
    endpoints: provider.endpoints.map((saved) => ({
      saved,
      slug: saved.slug,
      baseUrl: saved.baseUrl,
      protocol: saved.protocol,
      imageBaseUrl: saved.imageBaseUrl,
      imageEditBaseUrl: saved.imageEditBaseUrl,
      headers: saved.headers,
      models: saved.models,
    })),
  }
}

export function newEndpoint(taken: readonly EndpointDraft[]): EndpointDraft {
  return {
    saved: null,
    slug: nextSlug(taken),
    baseUrl: '',
    protocol: 'openai-chat',
    imageBaseUrl: null,
    imageEditBaseUrl: null,
    headers: {},
    models: [],
  }
}

/** First free `ep-N`; slugs are unique per provider and hidden from users. */
export function nextSlug(taken: readonly EndpointDraft[]): string {
  const used = new Set(taken.map((endpoint) => endpoint.slug))
  let n = taken.length + 1
  while (used.has(`ep-${n}`)) n++
  return `ep-${n}`
}

export const validUrl = (url: string): boolean => /^https?:\/\/\S+$/.test(url.trim())

/** Endpoints are stored even without an address (keys hang on them); only a malformed address stays local. */
export function savable(draft: ProviderDraft): ProviderDraft {
  return {
    ...draft,
    endpoints: draft.endpoints.filter((e) => e.saved !== null || !e.baseUrl.trim() || validUrl(e.baseUrl)),
  }
}

/**
 * Takes the stored provider after a save without discarding what the user typed meanwhile:
 * sent endpoints get their stored record (new ones matched by slug); edits stay in the draft.
 */
export function adopt(current: ProviderDraft, sent: ProviderDraft, stored: Provider): ProviderDraft {
  const sentSlugs = new Set(sent.endpoints.map((e) => e.slug))
  return {
    ...current,
    id: stored.id,
    endpoints: current.endpoints.map((endpoint) => {
      const record =
        stored.endpoints.find((e) => e.id === endpoint.saved?.id) ??
        (sentSlugs.has(endpoint.slug) ? stored.endpoints.find((e) => e.slug === endpoint.slug) : undefined)
      return record ? { ...endpoint, saved: record } : endpoint
    }),
  }
}

export function toSave(draft: ProviderDraft): ProviderSave {
  const endpoints = draft.endpoints.map((e): EndpointSave => {
    const save: EndpointSave = {
      slug: e.slug.trim(),
      baseUrl: e.baseUrl.trim(),
      protocol: e.protocol,
      imageBaseUrl: e.imageBaseUrl?.trim() || null,
      imageEditBaseUrl: e.imageEditBaseUrl?.trim() || null,
      headers: e.headers,
      models: e.models.map((model) => ({
        ...model,
        nativeWebSearch: model.modelType === 'chat' && e.protocol !== 'openai-chat' && model.nativeWebSearch,
      })),
    }
    if (e.saved) save.id = e.saved.id
    return save
  })
  const save = { name: draft.name.trim(), enabled: draft.enabled, notes: draft.notes, endpoints }
  return draft.id === null ? save : { id: draft.id, ...save }
}

/** Always a draft target: with endpointId the backend uses the stored keys on an edited, unsaved address. */
export function probeTarget(e: EndpointDraft, keyId?: string): ProbeTarget {
  const target: ProbeTarget = { kind: 'draft', baseUrl: e.baseUrl.trim(), protocol: e.protocol }
  if (e.saved) target.endpointId = e.saved.id
  if (keyId) target.keyId = keyId
  return target
}

const apiBase = (baseUrl: string): string => {
  const trimmed = baseUrl.trim().replace(/\/+$/, '')
  return trimmed.endsWith('/v1') ? trimmed : `${trimmed}/v1`
}

/** Mirrors `aether_providers::api_base` + `/models` for the `→ GET` line before saving. */
export const probeUrl = (baseUrl: string): string => `${apiBase(baseUrl)}/models`

const CHAT_PATH: Record<Protocol, string> = {
  'openai-chat': 'chat/completions',
  'openai-responses': 'responses',
  'anthropic-messages': 'messages',
}
/** Mirrors `aether_chat::protocol` request paths. */
export const chatUrl = (baseUrl: string, protocol: Protocol): string => `${apiBase(baseUrl)}/${CHAT_PATH[protocol]}`

const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]{1,100}$/
const RESERVED_HEADERS: Record<string, true> = {
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
export type HeadersError = { key: 'headers_too_many' | 'headers_invalid' } | { key: 'headers_reserved'; name: string }

/** Front-end mirror of the backend header rules (docs/36 §3.3); blank rows are dropped. */
export function checkHeaders(
  rows: readonly { name: string; value: string }[],
): { headers: Record<string, string> } | { error: HeadersError } {
  const kept = rows.map((r) => ({ name: r.name.trim(), value: r.value })).filter((r) => r.name || r.value)
  if (kept.length > 20) return { error: { key: 'headers_too_many' } }
  const headers: Record<string, string> = {}
  const seen = new Set<string>()
  for (const { name, value } of kept) {
    const lower = name.toLowerCase()
    if (Object.hasOwn(RESERVED_HEADERS, lower)) return { error: { key: 'headers_reserved', name } }
    if (!HEADER_NAME.test(name) || seen.has(lower) || value.length > 2048 || /[\r\n]/.test(value))
      return { error: { key: 'headers_invalid' } }
    seen.add(lower)
    headers[name] = value
  }
  return { headers }
}

/** Cherry's primary key: the first enabled one, else the first. */
export const primaryKey = (keys: readonly EndpointKey[]): EndpointKey | null =>
  keys.find((k) => k.enabled) ?? keys[0] ?? null

export const maskedText = (key: EndpointKey, t: KitI18n['t']): string =>
  key.readable && key.masked ? key.masked.replace(/\*{4,}/, '****') : t('providers.key_unreadable')

/** Maps key save errors (docs/34 §3.2) to their texts. */
export function keyError(error: unknown, t: KitI18n['t']): string {
  if (!isKitClientError(error)) return t('providers.keys_save_failed')
  const { field, reason } = error.details
  if (reason === 'duplicate') return t('providers.keys_err_duplicate')
  if (reason === 'too_many') return t('providers.keys_err_too_many')
  if (field === 'label' && reason === 'too_long') return t('providers.keys_err_label')
  if (field === 'apiKey' && reason === 'empty') return t('providers.keys_err_empty')
  return error.code === 'bad_request' ? t('providers.keys_save_failed') : t(`errors.${error.code}`)
}
