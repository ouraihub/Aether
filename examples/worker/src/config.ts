import { KitFailure } from '@ouraihub/aether-core/error'
import type { ToolSpec } from '@ouraihub/aether-core/types'
import {
  createCredentialPort,
  importCredentialKey,
} from '@ouraihub/aether-worker'
import type {
  Actor,
  CredentialPort,
  D1Database,
  DurableObjectNamespace,
  FetchBinding,
  R2Bucket,
} from '@ouraihub/aether-worker'
/**
 * Unknown deployment bindings are validated before constructing adapters.
 */
export interface Env {
  AETHER_DB: unknown
  AETHER_FILES: unknown
  RUNS: unknown
  OUTBOUND_GATEWAY?: unknown
  TARGET_CHECKER?: unknown
  APPLICATION_ID: string
  APP_ORIGIN: string
  TEST_MODE?: string
  ALLOW_LOOPBACK_HTTP?: string
  ALLOW_LOOPBACK_OUTBOUND?: string
  OUTBOUND_ORIGINS: string
  CREDENTIAL_CURRENT_KEY: string
  CREDENTIAL_KEYS: string
  SMOKE_TOKEN?: string
  SMOKE_TOKEN2?: string
  IDENTITY_TOKEN?: string
  IDENTITY_TOKEN2?: string
  REAUTH_TOKEN?: string
  MARKER_A: string
  MARKER_B?: string
  DEFAULT_ENDPOINT_ID?: string
  DEFAULT_MODEL_ID?: string
  PAINTING_ENDPOINT_ID?: string
  PAINTING_MODEL_ID?: string
}
export function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
export function database(value: unknown): value is D1Database {
  return object(value) && typeof value.prepare === 'function'
}
export function bucket(value: unknown): value is R2Bucket {
  return (
    object(value) &&
    typeof value.put === 'function' &&
    typeof value.get === 'function' &&
    typeof value.delete === 'function'
  )
}
export function namespace(value: unknown): value is DurableObjectNamespace {
  return (
    object(value) &&
    typeof value.idFromName === 'function' &&
    typeof value.get === 'function'
  )
}
export function fetchBinding(value: unknown): value is FetchBinding {
  return object(value) && typeof value.fetch === 'function'
}
export function configured(message: string): never {
  throw new KitFailure('needs_config', message)
}
export function secret(value: string | undefined, name: string): string {
  if (!value || value.length < 32 || value.length > 4096 || /\s/u.test(value))
    configured(`${name} must be a configured high-entropy secret`)
  return value
}
export function equal(left: string, right: string): boolean {
  let difference = left.length ^ right.length
  for (let i = 0; i < Math.max(left.length, right.length); i++)
    difference |= (left.charCodeAt(i) || 0) ^ (right.charCodeAt(i) || 0)
  return difference === 0
}
export function origins(value: string): readonly string[] {
  const parsed: unknown = JSON.parse(value)
  if (!Array.isArray(parsed) || parsed.length === 0)
    configured('OUTBOUND_ORIGINS must be a nonempty JSON array')
  const result: string[] = []
  for (const item of parsed) {
    if (typeof item !== 'string' || new URL(item).origin !== item)
      configured('OUTBOUND_ORIGINS must contain exact origins')
    result.push(item)
  }
  return result
}
export function credentialPort(env: Env): CredentialPort {
  const keys = async () => {
    const parsed: unknown = JSON.parse(env.CREDENTIAL_KEYS)
    if (!object(parsed)) configured('CREDENTIAL_KEYS must be a JSON key ring')
    const imported = new Map<string, CryptoKey>()
    for (const [id, encoded] of Object.entries(parsed)) {
      if (typeof encoded !== 'string')
        configured('CREDENTIAL_KEYS entries must be base64 keys')
      imported.set(id, await importCredentialKey(encoded))
    }
    return createCredentialPort({
      currentKeyId: env.CREDENTIAL_CURRENT_KEY,
      keys: imported,
    })
  }
  let imported: Promise<CredentialPort> | undefined
  const load = () => (imported ??= keys())
  return {
    async seal(scope, value) {
      return (await load()).seal(scope, value)
    },
    async open(scope, value) {
      return (await load()).open(scope, value)
    },
    async rotate(scope, value) {
      return (await load()).rotate(scope, value)
    },
  }
}
export const readSchema = {
  $id: 'https://aether.example/tools/read-marker',
  type: 'object',
  properties: {},
  additionalProperties: false,
}
export const writeSchema = {
  $id: 'https://aether.example/tools/write-marker',
  type: 'object',
  properties: { value: { type: 'string', minLength: 1, maxLength: 256 } },
  required: ['value'],
  additionalProperties: false,
}
export const predicates: Readonly<
  Record<string, (args: Readonly<Record<string, unknown>>) => boolean>
> = {
  read_marker: (args) => Object.keys(args).length === 0,
  write_marker: (args) =>
    Object.keys(args).length === 1 &&
    typeof args.value === 'string' &&
    args.value.length >= 1 &&
    args.value.length <= 256,
}
export const tools: readonly ToolSpec[] = [
  {
    name: 'read_marker',
    description:
      'Read the secret marker belonging to the current authenticated actor.',
    parameters: readSchema,
    approval: 'auto',
    effect: 'read',
    source: { kind: 'host' },
  },
  {
    name: 'write_marker',
    description:
      'Persist a marker value for the current actor. Requires user approval.',
    parameters: writeSchema,
    approval: 'ask',
    effect: 'edit',
    source: { kind: 'host' },
  },
]
export function authorized(actor: Actor, applicationId: string): boolean {
  return (
    actor.applicationId === applicationId &&
    (actor.subjectId === 'actor-a' || actor.subjectId === 'actor-b')
  )
}

