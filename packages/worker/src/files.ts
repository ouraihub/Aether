import { badRequest, KitFailure } from '@ouraihub/aether-core'
import { ownerKey, type Actor } from './context.ts'
import type { R2Bucket } from './cloudflare.ts'

export interface StoredFile {
  readonly id: string
  readonly mime: string
  readonly size: number
  readonly data: Uint8Array
}
export interface FileStore {
  put(
    actor: Actor,
    id: string,
    file: { readonly mime: string; readonly data: Uint8Array },
  ): Promise<StoredFile>
  get(actor: Actor, id: string): Promise<StoredFile | null>
  remove(actor: Actor, id: string): Promise<void>
  /** An authenticated application route, never an R2 public/presigned secret URL. */
  url(actor: Actor, id: string): string
}
const MIME_TYPES: Readonly<Record<string, true>> = {
  'image/png': true,
  'image/jpeg': true,
  'image/webp': true,
  'image/gif': true,
  'application/json': true,
  'application/octet-stream': true,
}
function fileId(id: string): void {
  if (!/^[A-Za-z0-9_-]{1,128}$/u.test(id)) throw badRequest('fileId', 'invalid')
}
function matchesMime(data: Uint8Array, mime: string): boolean {
  if (mime === 'image/png')
    return (
      data[0] === 137 &&
      data[1] === 80 &&
      data[2] === 78 &&
      data[3] === 71 &&
      data[4] === 13 &&
      data[5] === 10 &&
      data[6] === 26 &&
      data[7] === 10
    )
  if (mime === 'image/jpeg')
    return data[0] === 255 && data[1] === 216 && data[2] === 255
  if (mime === 'image/gif')
    return (
      new TextDecoder().decode(data.subarray(0, 6)) === 'GIF87a' ||
      new TextDecoder().decode(data.subarray(0, 6)) === 'GIF89a'
    )
  if (mime === 'image/webp')
    return (
      new TextDecoder().decode(data.subarray(0, 4)) === 'RIFF' &&
      new TextDecoder().decode(data.subarray(8, 12)) === 'WEBP'
    )
  return mime === 'application/json' || mime === 'application/octet-stream'
}
export class R2FileStore implements FileStore {
  readonly #bucket: R2Bucket
  readonly #maxBytes: number
  constructor(bucket: R2Bucket, options: { readonly maxBytes: number }) {
    if (!Number.isSafeInteger(options.maxBytes) || options.maxBytes <= 0)
      throw badRequest('fileLimit', 'invalid')
    this.#bucket = bucket
    this.#maxBytes = options.maxBytes
  }
  async #key(actor: Actor, id: string): Promise<string> {
    fileId(id)
    const digest = await crypto.subtle.digest(
      'SHA-256',
      new TextEncoder().encode(ownerKey(actor)),
    )
    const ownerHash = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, '0'),
    ).join('')
    return `aether/files/v1/${ownerHash}/${id}`
  }
  async put(
    actor: Actor,
    id: string,
    file: { readonly mime: string; readonly data: Uint8Array },
  ): Promise<StoredFile> {
    if (!MIME_TYPES[file.mime] || !matchesMime(file.data, file.mime))
      throw badRequest('mime', 'unsupported')
    if (file.data.byteLength === 0 || file.data.byteLength > this.#maxBytes)
      throw badRequest('file', 'too_large', this.#maxBytes)
    const key = await this.#key(actor, id)
    const written = await this.#bucket.put(key, file.data, {
      httpMetadata: { contentType: file.mime },
      onlyIf: { etagDoesNotMatch: '*' },
    })
    if (written === null)
      throw new KitFailure('conflict', 'File already exists')
    return { id, mime: file.mime, size: file.data.byteLength, data: file.data }
  }
  async get(actor: Actor, id: string): Promise<StoredFile | null> {
    const object = await this.#bucket.get(await this.#key(actor, id))
    if (!object) return null
    const mime = object.httpMetadata?.contentType
    if (
      !mime ||
      !MIME_TYPES[mime] ||
      !Number.isSafeInteger(object.size) ||
      object.size < 1 ||
      object.size > this.#maxBytes
    )
      throw new KitFailure('internal', 'Stored file is unreadable')
    const data = new Uint8Array(await object.arrayBuffer())
    if (data.byteLength !== object.size || !matchesMime(data, mime))
      throw new KitFailure('internal', 'Stored file is unreadable')
    return { id, mime, size: object.size, data }
  }
  async remove(actor: Actor, id: string): Promise<void> {
    await this.#bucket.delete(await this.#key(actor, id))
  }
  url(actor: Actor, id: string): string {
    ownerKey(actor)
    fileId(id)
    return `/api/v1/files/${encodeURIComponent(id)}`
  }
}
