import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import type { Actor } from '../context.js'
import type { StoredDocument } from '../storage.js'
import type { ApiKeyRecord, GrantRecord, McpServiceOptions, McpSettings } from './types.js'
import { equal, forbidden, hash, notFound, object, random, strings, text, unique, validActor } from './common.js'

interface KeyState {
  record: ApiKeyRecord
  hash: string
}
interface GrantState {
  record: GrantRecord
  refreshHash: string
  refreshExpiresAt: number
  generation: number
}
interface TokenIndex {
  owner: Actor
  id: string
  kind: 'key' | 'access'
  expiresAt: number | null
}
export interface Authentication {
  actor: Actor
  tokenId: string
  scopes: readonly string[]
}

function keyRecord(value: unknown): value is ApiKeyRecord {
  return (
    object(value) &&
    typeof value.id === 'string' &&
    typeof value.name === 'string' &&
    strings(value.scopes) &&
    typeof value.prefix === 'string' &&
    typeof value.createdAt === 'number' &&
    nullableTime(value.expiresAt) &&
    nullableTime(value.revokedAt)
  )
}
function grantRecord(value: unknown): value is GrantRecord {
  return (
    object(value) &&
    typeof value.id === 'string' &&
    typeof value.applicationId === 'string' &&
    (value.tenantId === null || typeof value.tenantId === 'string') &&
    typeof value.subjectId === 'string' &&
    typeof value.clientId === 'string' &&
    typeof value.redirectUri === 'string' &&
    strings(value.scopes) &&
    typeof value.createdAt === 'number' &&
    nullableTime(value.expiresAt) &&
    nullableTime(value.revokedAt)
  )
}
function nullableTime(value: unknown): value is number | null {
  return value === null || (typeof value === 'number' && Number.isFinite(value))
}
function readKey(document: StoredDocument): KeyState {
  const value = document.value
  if (!object(value) || !keyRecord(value.record) || typeof value.hash !== 'string')
    throw new KitFailure('internal', 'Invalid key storage')
  return { record: value.record, hash: value.hash }
}
function readGrant(document: StoredDocument): GrantState {
  const value = document.value
  if (
    !object(value) ||
    !grantRecord(value.record) ||
    typeof value.refreshHash !== 'string' ||
    typeof value.refreshExpiresAt !== 'number' ||
    typeof value.generation !== 'number'
  )
    throw new KitFailure('internal', 'Invalid grant storage')
  return {
    record: value.record,
    refreshHash: value.refreshHash,
    refreshExpiresAt: value.refreshExpiresAt,
    generation: value.generation,
  }
}

export class McpAuth {
  readonly system: Actor
  readonly options: McpServiceOptions
  constructor(options: McpServiceOptions) {
    this.options = options
    this.system = {
      subjectId: 'aether:mcp:registry',
      applicationId: options.applicationId,
      tenantId: null,
      kind: 'service',
      sessionId: null,
      scopes: [],
    }
  }
  now(): number {
    return this.options.now?.() ?? Date.now()
  }
  actor(actor: Actor): void {
    if (
      !validActor(actor, this.options.applicationId) ||
      actor.kind !== 'user' ||
      actor.subjectId === this.system.subjectId
    )
      forbidden()
  }
  requested(value: unknown, actor?: Actor): string[] {
    if (!strings(value) || value.length === 0 || value.length > 100) throw badRequest('scopes', 'invalid')
    const scopes = unique(value)
    if (
      scopes.some(
        (scope) => !this.options.scopes.some((known) => known.id === scope) || (actor && !actor.scopes.includes(scope)),
      )
    )
      forbidden()
    return scopes
  }
  async settings(owner: Actor): Promise<McpSettings> {
    const doc = await this.options.store.get(owner, 'mcp-settings', 'settings')
    if (!doc) return { enabled: false, enabledScopes: [] }
    if (!object(doc.value) || typeof doc.value.enabled !== 'boolean' || !strings(doc.value.enabledScopes))
      throw new KitFailure('internal', 'Invalid MCP settings')
    return { enabled: doc.value.enabled, enabledScopes: doc.value.enabledScopes }
  }
  async saveSettings(actor: Actor, value: Record<string, unknown>): Promise<McpSettings> {
    this.actor(actor)
    if (typeof value.enabled !== 'boolean' || !strings(value.enabledScopes)) throw badRequest('settings', 'invalid')
    const enabledScopes = value.enabledScopes.length ? this.requested(value.enabledScopes, actor) : []
    const current = await this.options.store.get(actor, 'mcp-settings', 'settings')
    const settings = { enabled: value.enabled, enabledScopes }
    await this.options.store.compareAndSet(actor, 'mcp-settings', 'settings', current?.revision ?? null, settings)
    return settings
  }
  async keys(actor: Actor): Promise<ApiKeyRecord[]> {
    this.actor(actor)
    const result: ApiKeyRecord[] = []
    let after: string | undefined
    while (true) {
      const page = await this.options.store.list(actor, 'mcp-keys', {
        ...(after === undefined ? {} : { after }),
        limit: 200,
      })
      result.push(...page.map((doc) => readKey(doc).record))
      if (page.length < 200) return result
      const next = page[page.length - 1]?.key
      if (!next || (after !== undefined && next <= after)) throw new KitFailure('internal', 'Invalid key pagination')
      after = next
    }
  }
  async grants(actor: Actor): Promise<GrantRecord[]> {
    this.actor(actor)
    const result: GrantRecord[] = []
    let after: string | undefined
    while (true) {
      const page = await this.options.store.list(actor, 'mcp-grants', {
        ...(after === undefined ? {} : { after }),
        limit: 200,
      })
      result.push(...page.map((doc) => readGrant(doc).record))
      if (page.length < 200) return result
      const next = page[page.length - 1]?.key
      if (!next || (after !== undefined && next <= after)) throw new KitFailure('internal', 'Invalid grant pagination')
      after = next
    }
  }
  async issueKey(
    actor: Actor,
    input: Record<string, unknown>,
    request: Request,
  ): Promise<{ token: string; record: ApiKeyRecord }> {
    this.actor(actor)
    const name = text(input.name, 'name', 128).trim()
    const scopes = this.requested(input.scopes, actor)
    if (!(await this.options.authorizeConsent(actor, scopes, request))) forbidden()
    const now = this.now()
    const expiresAt = input.expiresAt === undefined ? now + 90 * 86400000 : input.expiresAt
    if (expiresAt !== null && (typeof expiresAt !== 'number' || !Number.isFinite(expiresAt) || expiresAt <= now))
      throw badRequest('expiresAt', 'invalid')
    const id = crypto.randomUUID()
    const token = `aether_key_${random()}`
    const digest = await hash(token)
    const record: ApiKeyRecord = {
      id,
      name,
      scopes,
      prefix: `${token.slice(0, 16)}…`,
      createdAt: now,
      expiresAt,
      revokedAt: null,
    }
    await this.options.store.compareAndSet(actor, 'mcp-keys', id, null, { record, hash: digest })
    await this.index(digest, { owner: actor, id, kind: 'key', expiresAt })
    return { token, record }
  }
  async revoke(actor: Actor, kind: 'key' | 'grant', id: string): Promise<void> {
    this.actor(actor)
    const collection = kind === 'key' ? 'mcp-keys' : 'mcp-grants'
    const document = await this.options.store.get(actor, collection, id)
    if (!document) notFound()
    const state = kind === 'key' ? readKey(document) : readGrant(document)
    if (state.record.revokedAt !== null) return
    await this.options.store.compareAndSet(actor, collection, id, document.revision, {
      ...state,
      record: { ...state.record, revokedAt: this.now() },
    })
  }
  private async index(digest: string, value: TokenIndex): Promise<void> {
    await this.options.store.compareAndSet(this.system, 'mcp-token-index', digest, null, {
      ...value,
      audience: `${this.options.origin}/mcp`,
    })
  }
  async authenticate(token: string): Promise<Authentication | null> {
    if (token.length > 512) return null
    const digest = await hash(token)
    const doc = await this.options.store.get(this.system, 'mcp-token-index', digest)
    if (!doc || !object(doc.value)) return null
    const value = doc.value
    if (
      value.audience !== `${this.options.origin}/mcp` ||
      !validActor(value.owner, this.options.applicationId) ||
      value.owner.kind !== 'user' ||
      typeof value.id !== 'string' ||
      !nullableTime(value.expiresAt) ||
      (value.expiresAt !== null && value.expiresAt <= this.now())
    )
      return null
    const owner = value.owner
    let scopes: readonly string[]
    if (value.kind === 'key') {
      const key = await this.options.store.get(owner, 'mcp-keys', value.id)
      if (!key) return null
      const state = readKey(key)
      if (
        state.record.revokedAt !== null ||
        (state.record.expiresAt !== null && state.record.expiresAt <= this.now()) ||
        !equal(state.hash, digest)
      )
        return null
      scopes = state.record.scopes
    } else if (value.kind === 'access') {
      const grant = await this.options.store.get(owner, 'mcp-grants', value.id)
      if (!grant) return null
      const state = readGrant(grant)
      if (!this.activeGrant(state, owner)) return null
      scopes = state.record.scopes
    } else return null
    const settings = await this.settings(owner)
    if (!settings.enabled) return null
    const effective = scopes.filter(
      (scope) => settings.enabledScopes.includes(scope) && this.options.scopes.some((known) => known.id === scope),
    )
    return {
      actor: { ...owner, kind: 'mcp_client', sessionId: null, scopes: effective },
      tokenId: value.id,
      scopes: effective,
    }
  }
  private activeGrant(state: GrantState, owner: Actor): boolean {
    return (
      state.record.applicationId === owner.applicationId &&
      state.record.tenantId === owner.tenantId &&
      state.record.subjectId === owner.subjectId &&
      state.record.revokedAt === null &&
      (state.record.expiresAt === null || state.record.expiresAt > this.now())
    )
  }
  async createGrant(
    owner: Actor,
    clientId: string,
    redirectUri: string,
    scopes: readonly string[],
  ): Promise<GrantRecord> {
    const now = this.now()
    const record: GrantRecord = {
      id: crypto.randomUUID(),
      applicationId: owner.applicationId,
      tenantId: owner.tenantId,
      subjectId: owner.subjectId,
      clientId,
      redirectUri,
      scopes,
      createdAt: now,
      expiresAt: now + 30 * 86400000,
      revokedAt: null,
    }
    await this.options.store.compareAndSet(owner, 'mcp-grants', record.id, null, {
      record,
      refreshHash: '',
      refreshExpiresAt: 0,
      generation: 0,
    })
    return record
  }
  async issueTokens(
    owner: Actor,
    grantId: string,
    refresh?: string,
    clientId?: string,
  ): Promise<Record<string, unknown>> {
    const document = await this.options.store.get(owner, 'mcp-grants', grantId)
    if (!document) throw new KitFailure('unauthorized', 'Invalid grant')
    const state = readGrant(document)
    if (!this.activeGrant(state, owner) || (clientId !== undefined && state.record.clientId !== clientId))
      throw new KitFailure('unauthorized', 'Invalid grant')
    if (
      refresh !== undefined &&
      (state.refreshExpiresAt <= this.now() || !equal(state.refreshHash, await hash(refresh)))
    )
      throw new KitFailure('unauthorized', 'Invalid grant')
    const accessToken = `aether_access_${random()}`
    const refreshToken = `aether_refresh_${random()}`
    const now = this.now()
    const updated: GrantState = {
      ...state,
      refreshHash: await hash(refreshToken),
      refreshExpiresAt: Math.min(now + 30 * 86400000, state.record.expiresAt ?? Infinity),
      generation: state.generation + 1,
    }
    await this.options.store.compareAndSet(owner, 'mcp-grants', grantId, document.revision, updated)
    await this.index(await hash(accessToken), { owner, id: grantId, kind: 'access', expiresAt: now + 3600000 })
    await this.options.store.compareAndSet(this.system, 'mcp-refresh-index', await hash(refreshToken), null, {
      owner,
      grantId,
      clientId: state.record.clientId,
      audience: `${this.options.origin}/mcp`,
    })
    return {
      access_token: accessToken,
      token_type: 'Bearer',
      expires_in: 3600,
      refresh_token: refreshToken,
      scope: state.record.scopes.join(' '),
    }
  }
  async refresh(token: string, clientId: string): Promise<Record<string, unknown>> {
    const doc = await this.options.store.get(this.system, 'mcp-refresh-index', await hash(token))
    if (
      !doc ||
      !object(doc.value) ||
      doc.value.audience !== `${this.options.origin}/mcp` ||
      !validActor(doc.value.owner, this.options.applicationId) ||
      typeof doc.value.grantId !== 'string' ||
      doc.value.clientId !== clientId
    )
      throw new KitFailure('unauthorized', 'Invalid grant')
    return this.issueTokens(doc.value.owner, doc.value.grantId, token, clientId)
  }
}
