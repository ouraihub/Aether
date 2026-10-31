import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import type { Actor } from '../context.js'
import type { DocumentStore, StoredDocument } from '../storage.js'

export function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
export function strings(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.every((item: unknown) => typeof item === 'string')
  )
}
export function text(value: unknown, field: string, max = 1024): string {
  if (typeof value !== 'string' || !value.trim() || value.length > max)
    throw badRequest(field, 'invalid')
  return value
}
export function unique(values: readonly string[]): string[] {
  return [...new Set(values)]
}
export function random(): string {
  return base64url(crypto.getRandomValues(new Uint8Array(32)))
}
export function base64url(bytes: Uint8Array): string {
  let value = ''
  for (const byte of bytes) value += String.fromCharCode(byte)
  return btoa(value)
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replace(/=+$/u, '')
}
export async function hash(value: string): Promise<string> {
  return base64url(
    new Uint8Array(
      await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value)),
    ),
  )
}
export function equal(a: string, b: string): boolean {
  let difference = a.length ^ b.length
  for (let index = 0; index < Math.max(a.length, b.length); index++)
    difference |= (a.charCodeAt(index) || 0) ^ (b.charCodeAt(index) || 0)
  return difference === 0
}
export async function body(request: Request, max = 65536): Promise<string> {
  if (Number(request.headers.get('Content-Length')) > max)
    throw badRequest('body', 'too_large', max)
  if (!request.body) return ''
  const reader = request.body.getReader()
  const chunks: Uint8Array[] = []
  let size = 0
  try {
    while (true) {
      const next = await reader.read()
      if (next.done) break
      size += next.value.byteLength
      if (size > max) {
        await reader.cancel()
        throw badRequest('body', 'too_large', max)
      }
      chunks.push(next.value)
    }
  } finally {
    reader.releaseLock()
  }
  const bytes = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  } catch {
    throw badRequest('body', 'invalid_utf8')
  }
}
export async function jsonBody(
  request: Request,
  max?: number,
): Promise<Record<string, unknown>> {
  let value: unknown
  try {
    value = JSON.parse(await body(request, max))
  } catch (error) {
    if (error instanceof KitFailure) throw error
    throw badRequest('body', 'invalid_json')
  }
  if (!object(value)) throw badRequest('body', 'object_required')
  return value
}
export function json(
  value: unknown,
  status = 200,
  extra?: HeadersInit,
): Response {
  const headers = new Headers(extra)
  headers.set('Content-Type', 'application/json')
  headers.set('Cache-Control', 'no-store')
  headers.set('X-Content-Type-Options', 'nosniff')
  return new Response(JSON.stringify(value), { status, headers })
}
export function forbidden(): never {
  throw new KitFailure('forbidden', 'Access denied')
}
export function notFound(): never {
  throw new KitFailure('not_found', 'Record not found')
}
export function validActor(
  value: unknown,
  applicationId: string,
): value is Actor {
  return (
    object(value) &&
    value.applicationId === applicationId &&
    typeof value.subjectId === 'string' &&
    value.subjectId.length > 0 &&
    (value.kind === 'user' ||
      value.kind === 'service' ||
      value.kind === 'mcp_client') &&
    (value.tenantId === null || typeof value.tenantId === 'string') &&
    (value.sessionId === null || typeof value.sessionId === 'string') &&
    strings(value.scopes)
  )
}
export async function required(
  store: DocumentStore,
  actor: Actor,
  collection: string,
  key: string,
): Promise<StoredDocument> {
  const document = await store.get(actor, collection, key)
  if (!document) return notFound()
  return document
}
export function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}
