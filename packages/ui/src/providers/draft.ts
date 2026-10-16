// Editable form of a provider and its conversion to save/probe requests (docs/11 §3, §7).
import type { Endpoint, EndpointSave, Model, ProbeTarget, Protocol, Provider, ProviderSave } from '../ports'

export interface EndpointDraft {
  /** Stored endpoint, `null` for a new one. Its `id` must go back on save (docs/11 陷阱 1). */
  saved: Endpoint | null
  slug: string
  baseUrl: string
  protocol: Protocol
  /** `null` keeps the stored key, `''` clears it (explicit button only, docs/11 陷阱 2), else sets it. */
  key: string | null
  models: Model[]
}

export interface ProviderDraft {
  id: string | null
  name: string
  enabled: boolean
  endpoints: EndpointDraft[]
}

export function toDraft(provider: Provider): ProviderDraft {
  return {
    id: provider.id,
    name: provider.name,
    enabled: provider.enabled,
    endpoints: provider.endpoints.map((saved) => ({
      saved,
      slug: saved.slug,
      baseUrl: saved.baseUrl,
      protocol: saved.protocol,
      key: null,
      models: saved.models,
    })),
  }
}

export function newEndpoint(taken: readonly EndpointDraft[]): EndpointDraft {
  return { saved: null, slug: nextSlug(taken), baseUrl: '', protocol: 'openai-chat', key: null, models: [] }
}

/** First free `ep-N`; slugs are unique per provider and hidden from users. */
export function nextSlug(taken: readonly EndpointDraft[]): string {
  const used = new Set(taken.map((endpoint) => endpoint.slug))
  let n = taken.length + 1
  while (used.has(`ep-${n}`)) n++
  return `ep-${n}`
}

export const validUrl = (url: string): boolean => /^https?:\/\/\S+$/.test(url.trim())

/** New endpoints without a usable address stay local; the backend would reject them. */
export function savable(draft: ProviderDraft): ProviderDraft {
  return { ...draft, endpoints: draft.endpoints.filter((e) => e.saved !== null || validUrl(e.baseUrl)) }
}

/**
 * Takes the stored provider after a save without discarding what the user typed meanwhile:
 * sent endpoints get their stored record (new ones matched by slug), keys that were sent
 * return to "keep", and edits made after `sent` was taken stay in the draft.
 */
export function adopt(current: ProviderDraft, sent: ProviderDraft, stored: Provider): ProviderDraft {
  const sentKeys = new Map(sent.endpoints.map((e) => [e.slug, e.key]))
  return {
    ...current,
    id: stored.id,
    endpoints: current.endpoints.map((endpoint) => {
      const record =
        stored.endpoints.find((e) => e.id === endpoint.saved?.id) ??
        (sentKeys.has(endpoint.slug) ? stored.endpoints.find((e) => e.slug === endpoint.slug) : undefined)
      if (!record) return endpoint
      const key = sentKeys.get(endpoint.slug) === endpoint.key ? null : endpoint.key
      return { ...endpoint, saved: record, key }
    }),
  }
}

export function toSave(draft: ProviderDraft): ProviderSave {
  const endpoints = draft.endpoints.map((e): EndpointSave => {
    const save: EndpointSave = {
      slug: e.slug.trim(),
      baseUrl: e.baseUrl.trim(),
      protocol: e.protocol,
      models: e.models.map((model) => ({
        ...model,
        nativeWebSearch: model.modelType === 'chat' && e.protocol !== 'openai-chat' && model.nativeWebSearch,
      })),
    }
    if (e.saved) save.id = e.saved.id
    if (e.key !== null) save.apiKey = e.key
    return save
  })
  return draft.id === null
    ? { name: draft.name.trim(), enabled: draft.enabled, endpoints }
    : { id: draft.id, name: draft.name.trim(), enabled: draft.enabled, endpoints }
}

/** Always a draft target: with Keep + endpointId the backend uses the stored key on an edited, unsaved address. */
export function probeTarget(e: EndpointDraft): ProbeTarget {
  const target: ProbeTarget = { kind: 'draft', baseUrl: e.baseUrl.trim(), protocol: e.protocol }
  if (e.key !== null) target.apiKey = e.key
  if (e.saved) target.endpointId = e.saved.id
  return target
}

/** Mirrors `aether_providers::api_base` + `/models` for the `→ GET` line before saving. */
export function probeUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, '')
  return `${trimmed.endsWith('/v1') ? trimmed : `${trimmed}/v1`}/models`
}
