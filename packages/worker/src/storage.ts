import { badRequest, KitFailure } from '@ouraihub/aether-core'
import { ownerKey, type Actor } from './context.ts'
import type { D1Database, R2Bucket } from './cloudflare.ts'

export interface StoredDocument {
  readonly key: string
  readonly revision: number
  readonly value: unknown
}
export interface DocumentListOptions {
  readonly after?: string
  readonly limit?: number
}
export interface DocumentStore {
  get(owner: Actor, collection: string, key: string): Promise<StoredDocument | null>
  list(owner: Actor, collection: string, options?: DocumentListOptions): Promise<readonly StoredDocument[]>
  compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expectedRevision: number | null,
    value: unknown,
  ): Promise<StoredDocument>
  remove(owner: Actor, collection: string, key: string, expectedRevision: number): Promise<void>
}
const INLINE_BYTES = 256 * 1024
const MAX_LIST = 200
function identifier(value: string): void {
  if (!value || value.length > 256) throw badRequest('document', 'invalid_identifier')
}
function revision(value: number): void {
  if (!Number.isSafeInteger(value) || value < 1 || value >= Number.MAX_SAFE_INTEGER)
    throw badRequest('revision', 'invalid')
}
function scope(owner: Actor, collection: string): readonly [string, string, string, string, string] {
  identifier(collection)
  return [owner.applicationId, JSON.stringify(owner.tenantId), owner.subjectId, ownerKey(owner), collection]
}
export async function validateD1Schema(database: D1Database): Promise<void> {
  let row: unknown
  try {
    row = await database.prepare('SELECT version FROM aether_schema WHERE singleton = 1').first()
  } catch {
    throw new KitFailure('incompatible_db', 'Apply the supported Aether public migration before startup')
  }
  if (typeof row !== 'object' || row === null || !('version' in row) || row.version !== 1) {
    throw new KitFailure('incompatible_db', 'Apply the supported Aether public migration before startup')
  }
}
/** D1 rows stay below its 2,000,000 byte limit. Large functional documents live
 * in immutable scoped R2 blobs; credentials inside documents MUST already be sealed.
 * Tombstones reserve deleted keys. Superseded/uncertain-commit blobs stay for explicit
 * deployment GC: deleting them inline would race readers or a committed SQL response.
 */
export class D1DocumentStore implements DocumentStore {
  readonly #database: D1Database
  readonly #blobs: R2Bucket | undefined
  readonly #maxBytes: number
  constructor(database: D1Database, options: { readonly blobs?: R2Bucket; readonly maxDocumentBytes?: number } = {}) {
    this.#database = database
    this.#blobs = options.blobs
    this.#maxBytes = options.maxDocumentBytes ?? 64 * 1024 * 1024
    if (!Number.isSafeInteger(this.#maxBytes) || this.#maxBytes < INLINE_BYTES)
      throw badRequest('maxDocumentBytes', 'invalid')
  }
  async #blobPrefix(owner: Actor): Promise<string> {
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(ownerKey(owner)))
    return `aether/documents/v1/${Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('')}/`
  }
  async #parse(owner: Actor, row: unknown): Promise<StoredDocument> {
    if (
      typeof row !== 'object' ||
      row === null ||
      !('key' in row) ||
      typeof row.key !== 'string' ||
      !('revision' in row) ||
      typeof row.revision !== 'number' ||
      !Number.isSafeInteger(row.revision) ||
      row.revision < 1 ||
      !('value_json' in row) ||
      !('blob_key' in row)
    )
      throw new KitFailure('internal', 'Stored document is unreadable')
    let json: string
    if (typeof row.value_json === 'string' && row.blob_key === null) json = row.value_json
    else if (row.value_json === null && typeof row.blob_key === 'string' && this.#blobs) {
      const prefix = await this.#blobPrefix(owner)
      if (!row.blob_key.startsWith(prefix) || !/^[a-f0-9-]{36}$/u.test(row.blob_key.slice(prefix.length)))
        throw new KitFailure('internal', 'Stored document is unreadable')
      const blob = await this.#blobs.get(row.blob_key)
      if (!blob || blob.size > this.#maxBytes) throw new KitFailure('internal', 'Stored document is unreadable')
      const bytes = await blob.arrayBuffer()
      if (bytes.byteLength !== blob.size || bytes.byteLength > this.#maxBytes)
        throw new KitFailure('internal', 'Stored document is unreadable')
      json = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
    } else throw new KitFailure('internal', 'Stored document is unreadable')
    if (new TextEncoder().encode(json).byteLength > this.#maxBytes)
      throw new KitFailure('internal', 'Stored document is unreadable')
    try {
      const value: unknown = JSON.parse(json)
      return { key: row.key, revision: row.revision, value }
    } catch {
      throw new KitFailure('internal', 'Stored document is unreadable')
    }
  }
  async get(owner: Actor, collection: string, key: string): Promise<StoredDocument | null> {
    identifier(key)
    const row = await this.#database
      .prepare(
        'SELECT key, revision, value_json, blob_key FROM aether_documents WHERE application_id = ? AND tenant_key = ? AND subject_id = ? AND owner_key = ? AND collection = ? AND key = ? AND deleted = 0',
      )
      .bind(...scope(owner, collection), key)
      .first()
    return row === null ? null : this.#parse(owner, row)
  }
  async list(owner: Actor, collection: string, options: DocumentListOptions = {}): Promise<readonly StoredDocument[]> {
    const limit = options.limit ?? MAX_LIST
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > MAX_LIST) throw badRequest('limit', 'invalid', MAX_LIST)
    if (options.after !== undefined) identifier(options.after)
    const result = await this.#database
      .prepare(
        'SELECT key, revision, value_json, blob_key FROM aether_documents WHERE application_id = ? AND tenant_key = ? AND subject_id = ? AND owner_key = ? AND collection = ? AND deleted = 0 AND key > ? ORDER BY key LIMIT ?',
      )
      .bind(...scope(owner, collection), options.after ?? '', limit)
      .all()
    if (!result.success || !Array.isArray(result.results)) throw new KitFailure('internal', 'Document listing failed')
    const documents: StoredDocument[] = []
    // Sequential hydration bounds R2 concurrency/memory to one document at a time.
    for (const row of result.results) documents.push(await this.#parse(owner, row))
    return documents
  }
  async compareAndSet(
    owner: Actor,
    collection: string,
    key: string,
    expectedRevision: number | null,
    value: unknown,
  ): Promise<StoredDocument> {
    identifier(key)
    if (expectedRevision !== null) revision(expectedRevision)
    const values = scope(owner, collection)
    let json: string | undefined
    try {
      json = JSON.stringify(value)
    } catch {
      throw badRequest('document', 'invalid_json')
    }
    if (json === undefined) throw badRequest('document', 'invalid_json')
    const bytes = new TextEncoder().encode(json)
    if (bytes.byteLength > this.#maxBytes) throw badRequest('document', 'too_large', this.#maxBytes)
    let inline: string | null = json
    let blobKey: string | null = null
    if (bytes.byteLength > INLINE_BYTES) {
      if (!this.#blobs) throw new KitFailure('needs_config', 'Large documents require an R2 blob binding')
      blobKey = `${await this.#blobPrefix(owner)}${crypto.randomUUID()}`
      const written = await this.#blobs.put(blobKey, bytes, {
        httpMetadata: { contentType: 'application/json' },
        onlyIf: { etagDoesNotMatch: '*' },
      })
      if (written === null) throw new KitFailure('conflict', 'Document blob already exists')
      inline = null
    }
    const row =
      expectedRevision === null
        ? await this.#database
            .prepare(
              'INSERT INTO aether_documents (application_id, tenant_key, subject_id, owner_key, collection, key, revision, value_json, blob_key, deleted, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, 0, ?) ON CONFLICT DO NOTHING RETURNING key, revision, value_json, blob_key',
            )
            .bind(...values, key, inline, blobKey, Date.now())
            .first()
        : await this.#database
            .prepare(
              'UPDATE aether_documents SET revision = revision + 1, value_json = ?, blob_key = ?, updated_at = ? WHERE application_id = ? AND tenant_key = ? AND subject_id = ? AND owner_key = ? AND collection = ? AND key = ? AND revision = ? AND deleted = 0 RETURNING key, revision, value_json, blob_key',
            )
            .bind(inline, blobKey, Date.now(), ...values, key, expectedRevision)
            .first()
    if (row === null) {
      if (blobKey && this.#blobs) await this.#blobs.delete(blobKey)
      throw new KitFailure('conflict', 'Document revision conflict')
    }
    // SQL RETURNING is authoritative; do not reread a row that another writer can change.
    if (typeof row !== 'object' || !('revision' in row) || typeof row.revision !== 'number')
      throw new KitFailure('internal', 'Stored document is unreadable')
    const parsed: unknown = JSON.parse(json)
    return { key, revision: row.revision, value: parsed }
  }
  async remove(owner: Actor, collection: string, key: string, expectedRevision: number): Promise<void> {
    identifier(key)
    revision(expectedRevision)
    const row = await this.#database
      .prepare(
        'UPDATE aether_documents SET revision = revision + 1, value_json = NULL, blob_key = NULL, deleted = 1, updated_at = ? WHERE application_id = ? AND tenant_key = ? AND subject_id = ? AND owner_key = ? AND collection = ? AND key = ? AND revision = ? AND deleted = 0 RETURNING key',
      )
      .bind(Date.now(), ...scope(owner, collection), key, expectedRevision)
      .first()
    if (row === null) throw new KitFailure('conflict', 'Document revision conflict')
  }
}
