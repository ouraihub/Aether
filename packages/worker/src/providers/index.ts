import {
  isContract,
  type Endpoint,
  type EndpointKeySave,
  type Model,
  type ModelFilter,
  type ModelOption,
  type ModelRef,
  type ModelType,
  type ProbeResult,
  type ProbeTarget,
  type Provider,
  type ProviderSave,
} from '@ouraihub/aether-contracts'
import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import { inferReasoning } from '@ouraihub/aether-core/providers/infer'
import { probe as probeEndpoint } from '@ouraihub/aether-core/providers/probe'
import {
  prepare,
  validateHeaders,
  validateModel,
  validateUrl,
  apiBase,
  charCount,
  type EndpointSnapshot,
} from '@ouraihub/aether-core/providers/connection'
import type { Connection } from '@ouraihub/aether-core/types'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import { ownerKey, type Actor, type WorkerContext } from '../context.js'
import type { CredentialPort, CredentialScope, SealedCredential } from '../credentials.js'
import type { DocumentStore, StoredDocument } from '../storage.js'
import { parseCatalog, type Catalog, type Secret } from './catalog.js'
import { modelOptions } from './model-options.js'
import { handleProviderRequest } from './http.js'

export interface ProviderServiceOptions {
  documents: DocumentStore
  credentials: CredentialPort
  outbound: OutboundHttp
  authorizeReveal: (request: Request, context: WorkerContext) => Promise<void>
}

const missing = (entity: string): KitFailure =>
  new KitFailure('not_found', `Unknown ${entity}`, { entity })
const reasoningKey = (endpointId: string, modelId: string): string =>
  JSON.stringify([endpointId, modelId])
export class ProviderService {
  private readonly cursors = new Map<string, number>()
  private readonly options: ProviderServiceOptions
  constructor(options: ProviderServiceOptions) {
    this.options = options
  }
  private scope(actor: Actor, keyId: string): CredentialScope {
    return {
      applicationId: actor.applicationId,
      tenantId: actor.tenantId,
      subjectId: actor.subjectId,
      purpose: 'provider',
      recordId: keyId,
    }
  }
  private async load(actor: Actor): Promise<{ document: StoredDocument | null; catalog: Catalog }> {
    const document = await this.options.documents.get(actor, 'providers', 'catalog')
    return {
      document,
      catalog: document
        ? parseCatalog(document.value)
        : { providers: [], secrets: [], inferred: [] },
    }
  }
  private async commit(
    actor: Actor,
    document: StoredDocument | null,
    catalog: Catalog,
  ): Promise<Provider[]> {
    await this.options.documents.compareAndSet(
      actor,
      'providers',
      'catalog',
      document?.revision ?? null,
      catalog,
    )
    return this.list(actor)
  }
  private endpoint(
    catalog: Catalog,
    endpointId: string,
  ): { provider: Provider; endpoint: Endpoint } {
    for (const provider of catalog.providers) {
      const endpoint = provider.endpoints.find((item) => item.id === endpointId)
      if (endpoint) return { provider, endpoint }
    }
    throw missing('endpoint')
  }
  private async open(actor: Actor, secret: Secret): Promise<string> {
    const bytes = await this.options.credentials.open(
      this.scope(actor, secret.keyId),
      secret.sealed,
    )
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(bytes)
    } finally {
      bytes.fill(0)
    }
  }
  async list(actor: Actor): Promise<Provider[]> {
    const { catalog } = await this.load(actor)
    for (const provider of catalog.providers)
      for (const endpoint of provider.endpoints)
        for (const key of endpoint.keys) {
          const secret = catalog.secrets.find(
            (item) => item.endpointId === endpoint.id && item.keyId === key.id,
          )
          let plain: string | null = null
          if (secret)
            try {
              plain = await this.open(actor, secret)
            } catch {
              /* Unreadable credentials remain visible but cannot execute. */
            }
          key.readable = plain !== null
          key.masked =
            plain === null
              ? null
              : plain.length > 8
                ? `${plain.slice(0, 4)}…${plain.slice(-4)}`
                : '********'
        }
    return catalog.providers
  }
  async save(actor: Actor, input: ProviderSave): Promise<Provider[]> {
    if (!isContract('ProviderSave', input)) throw badRequest('provider', 'invalid')
    const { document, catalog } = await this.load(actor)
    const name = input.name.trim()
    if (!name || charCount(name) > 80) throw badRequest('name', 'invalid')
    if (input.notes !== undefined && charCount(input.notes) > 10000)
      throw badRequest('notes', 'too_long', 10000)
    if (input.endpoints.length > 20) throw badRequest('endpoints', 'too_many', 20)
    if (input.presetId && !input.id) {
      if (catalog.providers.some((item) => item.presetId === input.presetId))
        return this.list(actor)
      const adopted = catalog.providers.find((item) => item.name === name && item.presetId === null)
      if (adopted) {
        adopted.presetId = input.presetId
        return this.commit(actor, document, catalog)
      }
    }
    const old = input.id ? catalog.providers.find((item) => item.id === input.id) : undefined
    if (input.id && !old) throw missing('provider')
    if (input.presetId && old && input.presetId !== old.presetId)
      throw badRequest('presetId', 'immutable')
    const provider: Provider = {
      id: old?.id ?? crypto.randomUUID(),
      name,
      enabled: old?.enabled ?? !input.presetId,
      notes: input.notes ?? old?.notes ?? '',
      presetId: old?.presetId ?? input.presetId ?? null,
      endpoints: [],
    }
    if (input.enabled !== undefined && !(!old && input.presetId)) provider.enabled = input.enabled
    const slugs = new Set<string>()
    const endpointIds = new Set<string>()
    const inferred = new Set(catalog.inferred)
    for (const saved of input.endpoints) {
      if (!/^[a-z0-9-]{1,40}$/.test(saved.slug) || slugs.has(saved.slug))
        throw badRequest('slug', 'invalid')
      slugs.add(saved.slug)
      const previous = saved.id ? old?.endpoints.find((item) => item.id === saved.id) : undefined
      if (saved.id && !previous) throw missing('endpoint')
      const id = previous?.id ?? crypto.randomUUID()
      if (endpointIds.has(id)) throw badRequest('endpoints', 'duplicate')
      endpointIds.add(id)
      validateUrl('baseUrl', saved.baseUrl)
      const override = (field: 'imageBaseUrl' | 'imageEditBaseUrl'): string | null => {
        const value = saved[field] === undefined ? (previous?.[field] ?? null) : saved[field]
        if (value) validateUrl(field, value)
        return value?.trim() || null
      }
      if (saved.models.length > 1000) throw badRequest('models', 'too_many', 1000)
      const models: Model[] = []
      const modelIds = new Set<string>()
      for (const savedModel of saved.models) {
        if (modelIds.has(savedModel.id)) throw badRequest('models', 'duplicate')
        modelIds.add(savedModel.id)
        const prior = previous?.models.find((item) => item.id === savedModel.id)
        const key = reasoningKey(id, savedModel.id)
        const infer = savedModel.reasoning === undefined && (!prior || inferred.has(key))
        const reasoning =
          savedModel.reasoning === undefined
            ? infer
              ? savedModel.modelType === 'chat'
                ? inferReasoning(savedModel.id, saved.protocol, saved.baseUrl, savedModel.maxOutput)
                : null
              : (prior?.reasoning ?? null)
            : savedModel.reasoning
        if (infer) inferred.add(key)
        else inferred.delete(key)
        const model: Model = {
          ...savedModel,
          name: savedModel.name?.trim() || null,
          groupName: savedModel.groupName?.trim() || null,
          reasoning,
        }
        validateModel(model, saved.protocol)
        models.push(model)
      }
      const endpoint: Endpoint = {
        id,
        slug: saved.slug,
        baseUrl: saved.baseUrl.trim(),
        protocol: saved.protocol,
        keys: previous?.keys ?? [],
        keyStrategy: 'round_robin',
        probeUrl: `${apiBase(saved.baseUrl)}/models`,
        models,
        imageBaseUrl: override('imageBaseUrl'),
        imageEditBaseUrl: override('imageEditBaseUrl'),
        headers: validateHeaders(saved.headers ?? previous?.headers ?? {}),
      }
      provider.endpoints.push(endpoint)
    }
    catalog.providers = old
      ? catalog.providers.map((item) => (item.id === old.id ? provider : item))
      : [...catalog.providers, provider]
    const allEndpoints = new Set(
      catalog.providers.flatMap((item) => item.endpoints.map((endpoint) => endpoint.id)),
    )
    catalog.secrets = catalog.secrets.filter((item) => allEndpoints.has(item.endpointId))
    catalog.inferred = [...inferred].filter((key) =>
      catalog.providers.some((item) =>
        item.endpoints.some((endpoint) =>
          endpoint.models.some((model) => reasoningKey(endpoint.id, model.id) === key),
        ),
      ),
    )
    return this.commit(actor, document, catalog)
  }
  async remove(actor: Actor, id: string): Promise<Provider[]> {
    const { document, catalog } = await this.load(actor)
    const provider = catalog.providers.find((item) => item.id === id)
    if (!provider) throw missing('provider')
    if (provider.presetId) throw badRequest('provider', 'preset')
    catalog.providers = catalog.providers.filter((item) => item.id !== id)
    const ids = new Set(provider.endpoints.map((endpoint) => endpoint.id))
    catalog.secrets = catalog.secrets.filter((item) => !ids.has(item.endpointId))
    catalog.inferred = catalog.inferred.filter(
      (key) =>
        !provider.endpoints.some((endpoint) =>
          endpoint.models.some((model) => reasoningKey(endpoint.id, model.id) === key),
        ),
    )
    return this.commit(actor, document, catalog)
  }
  async setEnabled(actor: Actor, id: string, enabled: boolean): Promise<Provider[]> {
    const { document, catalog } = await this.load(actor)
    const provider = catalog.providers.find((item) => item.id === id)
    if (!provider) throw missing('provider')
    provider.enabled = enabled
    return this.commit(actor, document, catalog)
  }
  async saveKey(actor: Actor, input: EndpointKeySave): Promise<Provider[]> {
    if (!isContract('EndpointKeySave', input)) throw badRequest('key', 'invalid')
    const { document, catalog } = await this.load(actor)
    const { endpoint } = this.endpoint(catalog, input.endpointId)
    const old = input.id ? endpoint.keys.find((item) => item.id === input.id) : undefined
    if (input.id && !old) throw missing('key')
    if (!old && endpoint.keys.length >= 16) throw badRequest('keys', 'too_many', 16)
    if (!old && input.key === undefined) throw badRequest('apiKey', 'empty')
    const label = input.label?.trim() ?? old?.label ?? ''
    if (charCount(label) > 40) throw badRequest('label', 'too_long', 40)
    const id = old?.id ?? crypto.randomUUID()
    if (input.key !== undefined) {
      const plain = input.key.trim()
      if (!plain) throw badRequest('apiKey', 'empty')
      if (
        new TextEncoder().encode(plain).length > 4096 ||
        [...plain].some(
          (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
        )
      )
        throw badRequest('apiKey', 'too_long', 4096)
      for (const secret of catalog.secrets)
        if (secret.endpointId === endpoint.id && secret.keyId !== id) {
          let existing: string
          try {
            existing = await this.open(actor, secret)
          } catch {
            throw new KitFailure('needs_config', 'Credential cannot be read', {
              reason: 'key_unreadable',
            })
          }
          if (existing === plain) throw badRequest('apiKey', 'duplicate')
        }
      const bytes = new TextEncoder().encode(plain)
      let sealed: SealedCredential
      try {
        sealed = await this.options.credentials.seal(this.scope(actor, id), bytes)
      } finally {
        bytes.fill(0)
      }
      catalog.secrets = catalog.secrets.filter((item) => item.keyId !== id)
      catalog.secrets.push({ endpointId: endpoint.id, keyId: id, sealed })
    }
    const key = {
      id,
      label,
      enabled: input.enabled ?? old?.enabled ?? true,
      masked: null,
      readable: true,
      exhaustedUntil: input.restore ? null : (old?.exhaustedUntil ?? null),
      exhaustedReason: input.restore ? null : (old?.exhaustedReason ?? null),
    }
    endpoint.keys = old
      ? endpoint.keys.map((item) => (item.id === id ? key : item))
      : [...endpoint.keys, key]
    return this.commit(actor, document, catalog)
  }
  async removeKey(actor: Actor, endpointId: string, keyId: string): Promise<Provider[]> {
    const { document, catalog } = await this.load(actor)
    const { endpoint } = this.endpoint(catalog, endpointId)
    if (!endpoint.keys.some((item) => item.id === keyId)) throw missing('key')
    endpoint.keys = endpoint.keys.filter((item) => item.id !== keyId)
    catalog.secrets = catalog.secrets.filter(
      (item) => !(item.keyId === keyId && item.endpointId === endpointId),
    )
    return this.commit(actor, document, catalog)
  }
  async revealKey(actor: Actor, keyId: string): Promise<string> {
    const { catalog } = await this.load(actor)
    const secret = catalog.secrets.find((item) => item.keyId === keyId)
    if (!secret) throw missing('key')
    try {
      return await this.open(actor, secret)
    } catch {
      throw new KitFailure('needs_config', 'Credential cannot be read', {
        reason: 'key_unreadable',
      })
    }
  }
  private async snapshotFrom(
    actor: Actor,
    catalog: Catalog,
    endpointId: string,
    keyId?: string,
  ): Promise<EndpointSnapshot> {
    const { endpoint } = this.endpoint(catalog, endpointId)
    const chosen =
      keyId === undefined
        ? endpoint.keys.filter((key) => key.enabled)
        : endpoint.keys.filter((key) => key.id === keyId)
    if (keyId !== undefined && !chosen.length) throw missing('key')
    const keys: string[] = []
    for (const key of chosen) {
      const secret = catalog.secrets.find(
        (item) => item.keyId === key.id && item.endpointId === endpoint.id,
      )
      if (!secret)
        throw new KitFailure('needs_config', 'Credential cannot be read', {
          reason: 'key_unreadable',
        })
      try {
        keys.push(await this.open(actor, secret))
      } catch {
        throw new KitFailure('needs_config', 'Credential cannot be read', {
          reason: 'key_unreadable',
        })
      }
    }
    if (keyId === undefined && keys.length) {
      const cursorKey = JSON.stringify([ownerKey(actor), endpointId])
      const start = (this.cursors.get(cursorKey) ?? 0) % keys.length
      this.cursors.set(cursorKey, (start + 1) % keys.length)
      keys.push(...keys.splice(0, start))
    }
    return {
      endpointId,
      baseUrl: endpoint.baseUrl,
      protocol: endpoint.protocol,
      keys,
      headers: validateHeaders(endpoint.headers),
      imageBaseUrl: endpoint.imageBaseUrl,
      imageEditBaseUrl: endpoint.imageEditBaseUrl,
    }
  }
  async endpointSnapshot(actor: Actor, endpointId: string): Promise<EndpointSnapshot> {
    const { catalog } = await this.load(actor)
    return this.snapshotFrom(actor, catalog, endpointId)
  }
  async resolve(actor: Actor, reference: ModelRef, requiredType?: ModelType): Promise<Connection> {
    const { catalog } = await this.load(actor)
    const provider = catalog.providers.find(
      (item) =>
        item.enabled && item.endpoints.some((endpoint) => endpoint.id === reference.endpointId),
    )
    const endpoint = provider?.endpoints.find((item) => item.id === reference.endpointId)
    const model = endpoint?.models.find((item) => item.id === reference.modelId)
    if (!endpoint || !model) throw missing('model')
    if (requiredType !== undefined && model.modelType !== requiredType)
      throw badRequest('model', 'unsupported_type')
    prepare({
      endpoint: {
        endpointId: endpoint.id,
        baseUrl: endpoint.baseUrl,
        protocol: endpoint.protocol,
        keys: [],
        headers: validateHeaders(endpoint.headers),
        imageBaseUrl: endpoint.imageBaseUrl,
        imageEditBaseUrl: endpoint.imageEditBaseUrl,
      },
      model,
    })
    return prepare({ endpoint: await this.snapshotFrom(actor, catalog, endpoint.id), model })
  }
  async modelOptions(actor: Actor, filter: ModelFilter = {}): Promise<ModelOption[]> {
    const { catalog } = await this.load(actor)
    return modelOptions(catalog, filter)
  }
  async probe(actor: Actor, target: ProbeTarget, signal: AbortSignal): Promise<ProbeResult> {
    if (!isContract('ProbeTarget', target)) throw badRequest('target', 'invalid')
    const { catalog } = await this.load(actor)
    let endpoint: EndpointSnapshot
    if (target.kind === 'saved')
      endpoint = await this.snapshotFrom(actor, catalog, target.endpointId)
    else {
      if (target.keyId !== undefined && target.endpointId === undefined) throw missing('key')
      endpoint = target.endpointId
        ? {
            ...(await this.snapshotFrom(actor, catalog, target.endpointId, target.keyId)),
            baseUrl: target.baseUrl,
            protocol: target.protocol,
          }
        : {
            endpointId: null,
            baseUrl: target.baseUrl,
            protocol: target.protocol,
            keys: [],
            headers: {},
          }
    }
    return probeEndpoint(this.options.outbound, { endpoint, signal })
  }
  async handle(request: Request, context: WorkerContext): Promise<Response | null> {
    return handleProviderRequest(this, request, context, this.options.authorizeReveal)
  }
}
