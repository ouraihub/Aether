import { badRequest, KitFailure } from '@ouraihub/aether-core'
import { immutableActor, isActor, ownerKey, type Actor } from './context.ts'
import { retryCas } from './storage.ts'
import type { DocumentStore, StoredDocument } from './storage.ts'

export interface IdentityPort {
  resolve(request: Request): Promise<Actor | null>
  verifyFresh(request: Request, actor: Actor): Promise<boolean>
}
/** Login, cookies, CSRF and reauthentication remain product-owned callbacks. */
export function createIdentityPort(options: {
  readonly applicationId: string
  readonly resolve: (request: Request) => Promise<Actor | null>
  readonly verifyFresh: (request: Request, actor: Actor) => Promise<boolean>
}): IdentityPort {
  if (!options.applicationId) throw new KitFailure('needs_config', 'Identity application is required')
  return Object.freeze({
    async resolve(request: Request): Promise<Actor | null> {
      const actor = await options.resolve(request)
      return isActor(actor) && actor.applicationId === options.applicationId ? immutableActor(actor) : null
    },
    async verifyFresh(request: Request, actor: Actor): Promise<boolean> {
      if (!isActor(actor) || actor.applicationId !== options.applicationId) return false
      const current = await options.resolve(request)
      if (
        !isActor(current) ||
        ownerKey(current) !== ownerKey(actor) ||
        current.sessionId !== actor.sessionId ||
        current.kind !== actor.kind
      )
        return false
      // Never cache this result: each reveal invokes the host's fresh verification.
      return options.verifyFresh(request, immutableActor(current))
    },
  })
}
interface SessionRecord {
  readonly actor: Actor
  readonly createdAt: number
  readonly lastSeenAt: number
  readonly expiresAt: number
}
function parseSession(value: unknown): SessionRecord | null {
  if (
    typeof value !== 'object' ||
    value === null ||
    !('actor' in value) ||
    !isActor(value.actor) ||
    value.actor.kind !== 'user' ||
    value.actor.sessionId === null ||
    !('createdAt' in value) ||
    typeof value.createdAt !== 'number' ||
    !Number.isSafeInteger(value.createdAt) ||
    !('lastSeenAt' in value) ||
    typeof value.lastSeenAt !== 'number' ||
    !Number.isSafeInteger(value.lastSeenAt) ||
    !('expiresAt' in value) ||
    typeof value.expiresAt !== 'number' ||
    !Number.isSafeInteger(value.expiresAt) ||
    value.createdAt < 0 ||
    value.lastSeenAt < value.createdAt ||
    value.expiresAt <= value.lastSeenAt
  )
    return null
  return { actor: value.actor, createdAt: value.createdAt, lastSeenAt: value.lastSeenAt, expiresAt: value.expiresAt }
}
function opaqueToken(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) => byte.toString(16).padStart(2, '0')).join('')
}
async function tokenKey(token: string): Promise<string | null> {
  if (!/^[a-f0-9]{64}$/u.test(token)) return null
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(token))
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('')
}
/** Opaque browser sessions only; not OAuth bearer credentials or a login provider. */
export class OpaqueSessionStore {
  readonly #store: DocumentStore
  readonly #registry: Actor
  readonly #idleMs: number
  readonly #absoluteMs: number
  readonly #now: () => number
  constructor(options: {
    readonly store: DocumentStore
    readonly applicationId: string
    readonly idleMs: number
    readonly absoluteMs: number
    readonly now?: () => number
  }) {
    if (
      !options.applicationId ||
      !Number.isSafeInteger(options.idleMs) ||
      options.idleMs <= 0 ||
      !Number.isSafeInteger(options.absoluteMs) ||
      options.absoluteMs < options.idleMs
    )
      throw badRequest('session', 'invalid_configuration')
    this.#store = options.store
    this.#registry = immutableActor({
      applicationId: options.applicationId,
      tenantId: null,
      subjectId: 'aether:session-registry',
      kind: 'service',
      sessionId: null,
      scopes: [],
    })
    this.#idleMs = options.idleMs
    this.#absoluteMs = options.absoluteMs
    this.#now = options.now ?? Date.now
  }
  async issue(actor: Actor): Promise<{ readonly token: string; readonly actor: Actor; readonly expiresAt: number }> {
    if (!isActor(actor) || actor.kind !== 'user' || actor.applicationId !== this.#registry.applicationId)
      throw badRequest('actor', 'invalid_session_owner')
    const token = opaqueToken()
    const key = await tokenKey(token)
    if (key === null) throw new KitFailure('internal', 'Session generation failed')
    const now = this.#now()
    const sessionActor = immutableActor({ ...actor, sessionId: crypto.randomUUID() })
    const expiresAt = now + this.#absoluteMs
    await this.#store.compareAndSet(this.#registry, 'opaque_sessions', key, null, {
      actor: sessionActor,
      createdAt: now,
      lastSeenAt: now,
      expiresAt,
    })
    return { token, actor: sessionActor, expiresAt }
  }
  async #load(
    token: string,
  ): Promise<{ readonly key: string; readonly document: StoredDocument; readonly session: SessionRecord } | null> {
    const key = await tokenKey(token)
    if (key === null) return null
    const document = await this.#store.get(this.#registry, 'opaque_sessions', key)
    if (!document) return null
    const session = parseSession(document.value)
    const now = this.#now()
    if (
      !session ||
      session.actor.applicationId !== this.#registry.applicationId ||
      now < session.lastSeenAt ||
      now >= session.expiresAt ||
      now - session.lastSeenAt >= this.#idleMs
    )
      return null
    return { key, document, session }
  }
  async resolve(token: string): Promise<Actor | null> {
    const found = await this.#load(token)
    if (!found) return null
    try {
      await this.#store.compareAndSet(this.#registry, 'opaque_sessions', found.key, found.document.revision, {
        ...found.session,
        lastSeenAt: this.#now(),
      })
      return immutableActor(found.session.actor)
    } catch (error) {
      if (error instanceof KitFailure && error.code === 'conflict') return null
      throw error
    }
  }
  async revoke(token: string): Promise<void> {
    const key = await tokenKey(token)
    if (key === null) return
    // Retry only metadata CAS, never authentication or external side effects.
    await retryCas(
      async (cas) => {
        const document = await this.#store.get(this.#registry, 'opaque_sessions', key)
        if (document) await cas(() => this.#store.remove(this.#registry, 'opaque_sessions', key, document.revision))
      },
      () => new KitFailure('conflict', 'Session revocation conflicted'),
    )
  }
  async rotate(
    token: string,
  ): Promise<{ readonly token: string; readonly actor: Actor; readonly expiresAt: number } | null> {
    const found = await this.#load(token)
    if (!found) return null
    try {
      await this.#store.remove(this.#registry, 'opaque_sessions', found.key, found.document.revision)
    } catch (error) {
      if (error instanceof KitFailure && error.code === 'conflict') return null
      throw error
    }
    // Old token is already invalid even if issuing the replacement fails.
    const replacement = opaqueToken()
    const key = await tokenKey(replacement)
    if (key === null) throw new KitFailure('internal', 'Session generation failed')
    const actor = immutableActor({ ...found.session.actor, sessionId: crypto.randomUUID() })
    await this.#store.compareAndSet(this.#registry, 'opaque_sessions', key, null, {
      ...found.session,
      actor,
      lastSeenAt: this.#now(),
    })
    return { token: replacement, actor, expiresAt: found.session.expiresAt }
  }
}
