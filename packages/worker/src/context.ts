import { badRequest } from '@ouraihub/aether-core'

export interface Actor {
  readonly subjectId: string
  readonly kind: 'user' | 'service' | 'mcp_client'
  readonly applicationId: string
  readonly tenantId: string | null
  readonly sessionId: string | null
  readonly scopes: readonly string[]
}
export interface WorkerContext {
  readonly actor: Actor
  readonly signal: AbortSignal
  readonly requestId: string
  readonly now: number
}
export function isActor(value: unknown): value is Actor {
  if (typeof value !== 'object' || value === null) return false
  return (
    'subjectId' in value &&
    typeof value.subjectId === 'string' &&
    value.subjectId.length > 0 &&
    value.subjectId.length <= 256 &&
    'applicationId' in value &&
    typeof value.applicationId === 'string' &&
    value.applicationId.length > 0 &&
    value.applicationId.length <= 256 &&
    'kind' in value &&
    (value.kind === 'user' || value.kind === 'service' || value.kind === 'mcp_client') &&
    'tenantId' in value &&
    (value.tenantId === null ||
      (typeof value.tenantId === 'string' && value.tenantId.length > 0 && value.tenantId.length <= 256)) &&
    'sessionId' in value &&
    (value.sessionId === null ||
      (typeof value.sessionId === 'string' && value.sessionId.length > 0 && value.sessionId.length <= 256)) &&
    'scopes' in value &&
    Array.isArray(value.scopes) &&
    value.scopes.length <= 256 &&
    value.scopes.every((scope: unknown) => typeof scope === 'string' && scope.length > 0 && scope.length <= 256)
  )
}
export function immutableActor(actor: Actor): Actor {
  if (!isActor(actor)) throw badRequest('actor', 'invalid')
  return Object.freeze({ ...actor, scopes: Object.freeze([...actor.scopes]) })
}
export function ownerKey(actor: Actor): string {
  if (!isActor(actor)) throw badRequest('actor', 'invalid')
  return JSON.stringify([actor.applicationId, actor.tenantId, actor.subjectId])
}
