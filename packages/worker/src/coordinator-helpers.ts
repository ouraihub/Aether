import {
  isContract,
  type EndStatus,
  type KitError,
  type RunEvent,
  type RunSnapshot,
} from '@ouraihub/aether-contracts'
import { KitFailure } from '@ouraihub/aether-core'
import { immutableActor, isActor, type Actor } from './context.js'
import type { AcceptedHandle } from './conversations/index.js'

export type Phase = 'creating' | 'accepted' | 'running' | EndStatus
export interface Metadata {
  phase: Phase
  runId: string
  actor: Actor
  handle: AcceptedHandle | null
  snapshot: RunSnapshot
  revision: number
  acceptedUntil: number | null
  pending: { callId: string; revision: number } | null
}
export interface Pending {
  callId: string
  revision: number
  decision: boolean | undefined
  resolve: ((decision: boolean) => void) | null
}

export const storageKey = 'run'
export const encoder = new TextEncoder()
export const conflict = (reason: string): KitFailure =>
  new KitFailure('conflict', 'Run state conflict', { reason })
export const missing = (): KitFailure => new KitFailure('not_found', 'Run not found')
export const interrupted = (newId: () => string): KitError => ({
  code: 'internal',
  message: 'Run interrupted',
  details: { reason: 'interrupted', errorId: newId() },
})

/** No arbitrary messages/details cross this boundary, even from host KitFailures. */
export function publicError(error: unknown, newId: () => string): KitError {
  const code =
    error instanceof KitFailure
      ? error.code
      : isContract('KitError', error)
        ? error.code
        : 'internal'
  const messages: Record<KitError['code'], string> = {
    bad_request: 'Invalid request',
    not_found: 'Run not found',
    conflict: 'Run state conflict',
    unsupported: 'Unsupported operation',
    cancelled: 'Cancelled',
    unauthorized: 'Authentication required',
    forbidden: 'Access denied',
    rate_limited: 'Rate limit exceeded',
    upstream: 'Upstream execution failed',
    timeout: 'Execution timed out',
    network: 'Network execution failed',
    needs_config: 'Configuration required',
    budget_exceeded: 'Execution budget exceeded',
    incompatible_db: 'Incompatible database',
    internal: 'Internal execution failure',
  }
  // Error IDs are generated here, never copied from a potentially secret-bearing host error.
  const reason = error instanceof KitFailure ? error.details?.reason : undefined
  const safeReason =
    reason === 'event_replay_unavailable' ||
    reason === 'subscriber_already_active' ||
    reason === 'approval_not_pending'
      ? reason
      : undefined
  return {
    code,
    message: messages[code],
    ...(code === 'internal' || safeReason
      ? {
          details: {
            ...(safeReason ? { reason: safeReason } : {}),
            ...(code === 'internal' ? { errorId: newId() } : {}),
          },
        }
      : {}),
  }
}
export function statusCode(error: KitError): number {
  switch (error.code) {
    case 'bad_request':
      return 400
    case 'unauthorized':
      return 401
    case 'forbidden':
      return 403
    case 'not_found':
      return 404
    case 'conflict':
      return 409
    case 'rate_limited':
      return 429
    case 'internal':
    case 'incompatible_db':
      return 500
    default:
      return 422
  }
}
export function actorHeader(request: Request): Actor {
  try {
    const encoded = request.headers.get('x-aether-actor')
    if (!encoded || encoded.length > 131072) throw missing()
    const bytes = Uint8Array.from(atob(encoded), (character) => character.charCodeAt(0))
    const value: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
    if (!isActor(value)) throw missing()
    return immutableActor(value)
  } catch {
    throw missing()
  }
}
export function terminal(phase: Phase): phase is EndStatus {
  return phase === 'completed' || phase === 'failed' || phase === 'cancelled'
}
export function endEvent(snapshot: RunSnapshot): RunEvent {
  if (snapshot.status === 'running') throw conflict('run_not_terminal')
  return {
    kind: 'end',
    status: snapshot.status,
    ...(snapshot.error ? { error: snapshot.error } : {}),
    ...(snapshot.exhausted === undefined ? {} : { exhausted: snapshot.exhausted }),
  }
}
