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
  endpoints: EndpointDraft[]
}

export function toDraft(provider: Provider): ProviderDraft {
  return {
    id: provider.id,
    name: provider.name,
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

export function newEndpoint(index: number): EndpointDraft {
  return { saved: null, slug: `ep-${index + 1}`, baseUrl: '', protocol: 'openai-chat', key: null, models: [] }
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
    ? { name: draft.name.trim(), endpoints }
    : { id: draft.id, name: draft.name.trim(), endpoints }
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
