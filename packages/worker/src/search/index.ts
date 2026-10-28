import type {
  SearchCap,
  SearchKey,
  SearchPage,
  SearchPreferences,
  SearchProvider,
  SearchSettingsDto,
} from '@ouraihub/aether-contracts'
import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import {
  DEFAULT_PROVIDERS,
  PROVIDERS,
  defaultPreferences,
  fetch as fetchPage,
  providerFor,
  search as searchQuery,
  validatePreferences,
  validateQuery,
  validateTarget,
  type CheckTarget,
  type ResolvedSearchConfig,
} from '@ouraihub/aether-core/search'
import type { Actor, WorkerContext } from '../context.js'
import { ownerKey } from '../context.js'
import type { CredentialPort, CredentialScope, SealedCredential } from '../credentials.js'
import { isSealedCredential } from '../credentials.js'
import type { DocumentStore, StoredDocument } from '../storage.js'

interface KeyRecord {
  id: string
  sealed: SealedCredential
}
interface ProviderRecord {
  baseUrl?: string
  basicAuthUsername?: string
  keys: KeyRecord[]
}
export interface SearchServiceOptions {
  documents: DocumentStore
  credentials: CredentialPort
  outbound: OutboundHttp
  checkTarget: CheckTarget
  /** Must verify fresh host authentication; never automatically approve a reveal. */
  verifyCurrentPassword(actor: Actor, password: string, signal: AbortSignal): Promise<void>
}
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
function input(value: unknown, names: readonly string[]): Record<string, unknown> {
  if (!object(value) || Object.keys(value).some((name) => !names.includes(name)))
    throw badRequest('input', 'unknown_fields')
  return value
}
function string(value: unknown, field: string): string {
  if (typeof value !== 'string') throw badRequest(field, 'invalid')
  return value
}
function cap(value: unknown): SearchCap {
  if (value !== 'search' && value !== 'fetch') throw badRequest('capability', 'invalid')
  return value
}
function parseProvider(value: unknown): ProviderRecord {
  const data = input(value, ['baseUrl', 'basicAuthUsername', 'keys'])
  if (!Array.isArray(data.keys) || data.keys.length > 16)
    throw new KitFailure('needs_config', 'Invalid stored search credentials')
  const keys: KeyRecord[] = []
  for (const item of data.keys) {
    if (
      !object(item) ||
      typeof item.id !== 'string' ||
      !isSealedCredential(item.sealed) ||
      keys.some((key) => key.id === item.id)
    )
      throw new KitFailure('needs_config', 'Invalid stored search credentials')
    keys.push({ id: item.id, sealed: item.sealed })
  }
  return {
    keys,
    ...(data.baseUrl === undefined ? {} : { baseUrl: string(data.baseUrl, 'baseUrl') }),
    ...(data.basicAuthUsername === undefined
      ? {}
      : { basicAuthUsername: string(data.basicAuthUsername, 'basicAuthUsername') }),
  }
}
function scope(actor: Actor, capability: SearchCap, provider: string, keyId: string): CredentialScope {
  return {
    applicationId: actor.applicationId,
    tenantId: actor.tenantId,
    subjectId: actor.subjectId,
    purpose: 'search',
    recordId: JSON.stringify([capability, provider, keyId]),
  }
}
function partition(capability: SearchCap, provider: string): string {
  return JSON.stringify([capability, provider])
}
function mask(value: string): string {
  return value.length <= 4
    ? '••••'
    : value.slice(0, 2) + '•'.repeat(Math.min(12, Math.max(4, value.length - 4))) + value.slice(-2)
}

/** All encrypted keys and provider configuration share one CAS document per partition. */
export class SearchService {
  private readonly rotations = new Map<string, number>()
  private readonly options: SearchServiceOptions
  constructor(options: SearchServiceOptions) {
    this.options = options
  }
  private async stored(
    actor: Actor,
    capability: SearchCap,
    provider: string,
  ): Promise<{ document: StoredDocument | null; value: ProviderRecord }> {
    providerFor(capability, provider)
    const document = await this.options.documents.get(actor, 'search.providers', partition(capability, provider))
    return { document, value: document ? parseProvider(document.value) : { keys: [] } }
  }
  async preferences(actor: Actor): Promise<SearchPreferences> {
    const document = await this.options.documents.get(actor, 'search.preferences', 'global')
    return document ? validatePreferences(document.value) : defaultPreferences()
  }
  async savePreferences(actor: Actor, value: unknown): Promise<SearchPreferences> {
    const preferences = validatePreferences(value)
    const old = await this.options.documents.get(actor, 'search.preferences', 'global')
    await this.options.documents.compareAndSet(
      actor,
      'search.preferences',
      'global',
      old?.revision ?? null,
      preferences,
    )
    return preferences
  }
  private async selected(actor: Actor, capability: SearchCap): Promise<string> {
    const document = await this.options.documents.get(actor, 'search.defaults', capability)
    const provider = document
      ? string(input(document.value, ['provider']).provider, 'provider')
      : DEFAULT_PROVIDERS[capability]
    providerFor(capability, provider)
    return provider
  }
  private async open(actor: Actor, capability: SearchCap, provider: string, key: KeyRecord): Promise<string> {
    const bytes = await this.options.credentials.open(scope(actor, capability, provider, key.id), key.sealed)
    try {
      const value = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
      if (!value.trim() || value.length > 4096) throw new KitFailure('needs_config', 'Search credential unavailable')
      return value
    } finally {
      bytes.fill(0)
    }
  }
  async settings(actor: Actor, capability: SearchCap): Promise<SearchSettingsDto> {
    const providers: SearchProvider[] = []
    const order =
      capability === 'search'
        ? PROVIDERS.filter((item) => item.search).map((item) => item.id)
        : ['querit', 'fetch', 'jina', 'firecrawl']
    for (const id of order) {
      const { spec, config } = providerFor(capability, id)
      const { value } = await this.stored(actor, capability, id)
      let readable = 0
      const keys: SearchKey[] = []
      for (const key of value.keys) {
        try {
          keys.push({ id: key.id, masked: mask(await this.open(actor, capability, id, key)) })
          readable++
        } catch {
          keys.push({ id: key.id, masked: 'Unavailable' })
        }
      }
      providers.push({
        id,
        name: spec.name,
        description: spec.description,
        keyMode: config.keyMode,
        basicAuth: spec.basicAuth,
        ...(config.defaultBaseUrl ? { defaultBaseUrl: config.defaultBaseUrl } : {}),
        ...(value.baseUrl ? { baseUrl: value.baseUrl } : {}),
        ...(value.basicAuthUsername ? { basicAuthUsername: value.basicAuthUsername } : {}),
        keys,
        ready: (config.keyMode !== 'required' || readable > 0) && (!value.keys.length || readable > 0),
      })
    }
    return {
      capability,
      defaultProvider: await this.selected(actor, capability),
      preferences: await this.preferences(actor),
      providers,
    }
  }
  async save(actor: Actor, raw: unknown, signal: AbortSignal): Promise<SearchSettingsDto> {
    const data = input(raw, ['capability', 'provider', 'baseUrl', 'basicAuthUsername'])
    const capability = cap(data.capability)
    const provider = string(data.provider, 'provider')
    const { spec } = providerFor(capability, provider)
    const { document, value } = await this.stored(actor, capability, provider)
    if (data.baseUrl !== undefined) {
      const baseUrl = string(data.baseUrl, 'baseUrl').trim()
      if (new TextEncoder().encode(string(data.baseUrl, 'baseUrl')).length > 8192)
        throw badRequest('baseUrl', 'too_long', 8192)
      if (provider !== 'fetch' && baseUrl) await this.options.checkTarget(validateTarget(baseUrl), signal)
      delete value.baseUrl
      if (provider !== 'fetch' && baseUrl) value.baseUrl = baseUrl
    }
    if (data.basicAuthUsername !== undefined) {
      const username = string(data.basicAuthUsername, 'basicAuthUsername').trim()
      if (new TextEncoder().encode(username).length > 200) throw badRequest('basicAuthUsername', 'too_long', 200)
      if (!spec.basicAuth && username) throw badRequest('basicAuthUsername', 'unsupported')
      delete value.basicAuthUsername
      if (spec.basicAuth && username) value.basicAuthUsername = username
    }
    await this.options.documents.compareAndSet(
      actor,
      'search.providers',
      partition(capability, provider),
      document?.revision ?? null,
      value,
    )
    return this.settings(actor, capability)
  }
  async setDefault(actor: Actor, capability: SearchCap, provider: string): Promise<SearchSettingsDto> {
    providerFor(capability, provider)
    const old = await this.options.documents.get(actor, 'search.defaults', capability)
    await this.options.documents.compareAndSet(actor, 'search.defaults', capability, old?.revision ?? null, {
      provider,
    })
    return this.settings(actor, capability)
  }
  async saveKey(
    actor: Actor,
    capability: SearchCap,
    provider: string,
    raw: string,
    keyId?: string,
  ): Promise<SearchSettingsDto> {
    const { spec, config } = providerFor(capability, provider)
    if (config.keyMode === 'none' && !spec.basicAuth) throw badRequest('keys', 'unsupported')
    const plaintext = raw.trim()
    if (!plaintext || plaintext.length > 4096) throw badRequest('value', 'invalid', 4096)
    const { document, value } = await this.stored(actor, capability, provider)
    const existing = keyId === undefined ? undefined : value.keys.find((key) => key.id === keyId)
    if (keyId !== undefined && !existing) throw new KitFailure('not_found', 'Search key not found', { field: 'key' })
    if (!existing && value.keys.length >= 16) throw badRequest('keys', 'too_many', 16)
    const id = keyId ?? crypto.randomUUID()
    const bytes = new TextEncoder().encode(plaintext)
    let sealed: SealedCredential
    try {
      sealed = await this.options.credentials.seal(scope(actor, capability, provider, id), bytes)
    } finally {
      bytes.fill(0)
    }
    if (existing) existing.sealed = sealed
    else value.keys.push({ id, sealed })
    await this.options.documents.compareAndSet(
      actor,
      'search.providers',
      partition(capability, provider),
      document?.revision ?? null,
      value,
    )
    return this.settings(actor, capability)
  }
  async removeKey(actor: Actor, capability: SearchCap, provider: string, keyId: string): Promise<SearchSettingsDto> {
    const { document, value } = await this.stored(actor, capability, provider)
    if (!value.keys.some((key) => key.id === keyId))
      throw new KitFailure('not_found', 'Search key not found', { field: 'key' })
    value.keys = value.keys.filter((key) => key.id !== keyId)
    await this.options.documents.compareAndSet(
      actor,
      'search.providers',
      partition(capability, provider),
      document?.revision ?? null,
      value,
    )
    return this.settings(actor, capability)
  }
  async reveal(
    actor: Actor,
    capability: SearchCap,
    provider: string,
    keyId: string,
    password: string,
    signal: AbortSignal,
  ): Promise<string> {
    await this.options.verifyCurrentPassword(actor, password, signal)
    const { value } = await this.stored(actor, capability, provider)
    const key = value.keys.find((item) => item.id === keyId)
    if (!key) throw new KitFailure('not_found', 'Search key not found', { field: 'key' })
    try {
      return await this.open(actor, capability, provider, key)
    } catch {
      throw new KitFailure('needs_config', 'Search credential unavailable', { field: 'keys' })
    }
  }
  async rotateCredentials(actor: Actor, capability: SearchCap, provider: string): Promise<void> {
    const { document, value } = await this.stored(actor, capability, provider)
    if (!document) return
    for (const key of value.keys)
      key.sealed = await this.options.credentials.rotate(scope(actor, capability, provider, key.id), key.sealed)
    await this.options.documents.compareAndSet(
      actor,
      'search.providers',
      partition(capability, provider),
      document.revision,
      value,
    )
  }
  async resolve(actor: Actor, capability: SearchCap, explicitProvider?: string): Promise<ResolvedSearchConfig> {
    const provider = explicitProvider ?? (await this.selected(actor, capability))
    const { config } = providerFor(capability, provider)
    const { value } = await this.stored(actor, capability, provider)
    const preferences = await this.preferences(actor)
    const keys: string[] = []
    for (const key of value.keys) {
      try {
        keys.push(await this.open(actor, capability, provider, key))
      } catch {
        /* Unreadable keys are deliberately skipped. */
      }
    }
    if ((value.keys.length && !keys.length) || (config.keyMode === 'required' && !keys.length))
      throw new KitFailure('needs_config', 'Search credentials required', { field: 'keys' })
    const rotationKey = JSON.stringify([ownerKey(actor), capability, provider])
    const start = (this.rotations.get(rotationKey) ?? 0) % Math.max(1, keys.length)
    if (keys.length) this.rotations.set(rotationKey, (start + 1) % keys.length)
    return {
      provider,
      baseUrl: value.baseUrl || config.defaultBaseUrl,
      apiKeys: keys.slice(start).concat(keys.slice(0, start)),
      ...(value.basicAuthUsername ? { basicAuthUsername: value.basicAuthUsername } : {}),
      preferences,
    }
  }
  async search(actor: Actor, rawQuery: string, signal?: AbortSignal) {
    const query = validateQuery(rawQuery)
    return searchQuery(await this.resolve(actor, 'search'), query, this.options.outbound, signal)
  }
  async fetch(actor: Actor, rawUrl: string, signal?: AbortSignal): Promise<SearchPage> {
    const url = validateTarget(rawUrl)
    return fetchPage(await this.resolve(actor, 'fetch'), url, this.options.outbound, this.options.checkTarget, signal)
  }
  async check(actor: Actor, capability: SearchCap, provider: string, signal?: AbortSignal): Promise<{ count: number }> {
    const snapshot = await this.resolve(actor, capability, provider)
    if (capability === 'fetch') {
      await fetchPage(snapshot, 'https://example.com', this.options.outbound, this.options.checkTarget, signal)
      return { count: 1 }
    }
    const result = await searchQuery(snapshot, 'web search', this.options.outbound, signal)
    if (!result.hits.length) throw new KitFailure('upstream', 'Search provider returned no results')
    return { count: result.hits.length }
  }
  async handle(request: Request, context: WorkerContext): Promise<Response | null> {
    const url = new URL(request.url)
    if (!url.pathname.startsWith('/api/v1/search/')) return null
    const route = url.pathname.slice('/api/v1/search/'.length)
    const { actor, signal } = context
    if (route === 'settings' && request.method === 'GET') {
      if (
        [...url.searchParams.keys()].some((key) => key !== 'capability') ||
        url.searchParams.getAll('capability').length !== 1
      )
        throw badRequest('query', 'invalid')
      return Response.json(await this.settings(actor, cap(url.searchParams.get('capability'))))
    }
    const methods: Readonly<Record<string, string>> = {
      settings: 'POST',
      default: 'POST',
      preferences: 'PUT',
      keys: request.method === 'DELETE' ? 'DELETE' : 'POST',
      'keys/reveal': 'POST',
      check: 'POST',
      search: 'POST',
      fetch: 'POST',
    }
    if (!methods[route] || request.method !== methods[route]) throw badRequest('route', 'unsupported')
    if (url.search) throw badRequest('query', 'unsupported')
    let raw: unknown
    try {
      raw = await request.json()
    } catch {
      throw badRequest('body', 'invalid_json')
    }
    if (route === 'preferences') return Response.json(await this.savePreferences(actor, raw))
    if (route === 'settings') return Response.json(await this.save(actor, raw, signal))
    if (route === 'search') {
      const data = input(raw, ['query'])
      return Response.json(await this.search(actor, string(data.query, 'query'), signal))
    }
    if (route === 'fetch') {
      const data = input(raw, ['url'])
      return Response.json(await this.fetch(actor, string(data.url, 'url'), signal))
    }
    const names =
      route === 'keys/reveal'
        ? ['capability', 'provider', 'keyId', 'currentPassword']
        : route === 'keys'
          ? request.method === 'DELETE'
            ? ['capability', 'provider', 'keyId']
            : ['capability', 'provider', 'value', 'keyId']
          : ['capability', 'provider']
    const data = input(raw, names)
    const capability = cap(data.capability)
    const provider = string(data.provider, 'provider')
    if (route === 'default') return Response.json(await this.setDefault(actor, capability, provider))
    if (route === 'check') return Response.json(await this.check(actor, capability, provider, signal))
    if (route === 'keys/reveal')
      return Response.json(
        await this.reveal(
          actor,
          capability,
          provider,
          string(data.keyId, 'keyId'),
          string(data.currentPassword, 'currentPassword'),
          signal,
        ),
      )
    if (request.method === 'DELETE')
      return Response.json(await this.removeKey(actor, capability, provider, string(data.keyId, 'keyId')))
    return Response.json(
      await this.saveKey(
        actor,
        capability,
        provider,
        string(data.value, 'value'),
        data.keyId === undefined ? undefined : string(data.keyId, 'keyId'),
      ),
    )
  }
}
