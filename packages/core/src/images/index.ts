import type { ImagePart, KitErrorCode, Details } from '@ouraihub/aether-contracts'
import { badRequest, KitFailure } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import type { Connection } from '../types.js'

export const MAX_IMAGE_BYTES = 4 * 1024 * 1024
export const MAX_EDIT_IMAGES = 4
export const IMAGE_MIMES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'] as const
export const IMAGE_SIZES = ['1024x1024', '1024x1536', '1536x1024'] as const
export const IMAGE_TIMEOUT_MS = 120_000
const RESPONSE_BYTES = Math.ceil(MAX_IMAGE_BYTES / 3) * 4 + 64 * 1024
export type ImageSize = (typeof IMAGE_SIZES)[number]
export interface ImageRequest {
  readonly prompt: string
  readonly size: ImageSize
  readonly images: readonly ImagePart[]
}
export type ImageConnection = Pick<
  Connection,
  'baseUrl' | 'modelId' | 'keys' | 'headers' | 'canVision' | 'imageBaseUrl' | 'imageEditBaseUrl'
>
/** Unknown means a dispatched request may have generated an image: never retry it automatically. */
export type ImageDispatch = 'not_dispatched' | 'rejected' | 'unknown' | 'generated'
export class ImageFailure extends KitFailure {
  readonly dispatch: ImageDispatch
  constructor(code: KitErrorCode, message: string, dispatch: ImageDispatch, details?: Details) {
    super(code, message, details)
    this.dispatch = dispatch
    this.name = 'ImageFailure'
  }
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** Strict public ImagePart parsing, shared by direct consumers and run validation. */
export function parseImage(value: unknown): ImagePart {
  if (
    !record(value) ||
    Object.keys(value).some((key) => key !== 'mime' && key !== 'data') ||
    typeof value.mime !== 'string' ||
    typeof value.data !== 'string'
  ) {
    throw badRequest('images', 'encoding')
  }
  const image = { mime: value.mime, data: value.data }
  decodeImage(image)
  return image
}

/** Does not impose a chat or editing count policy. */
export function checkImages(images: unknown): asserts images is ImagePart[] {
  if (!Array.isArray(images)) throw badRequest('images', 'encoding')
  for (const image of images) parseImage(image)
}

export function decodeImage(image: ImagePart): Uint8Array<ArrayBuffer> {
  if (
    !record(image) ||
    Object.keys(image).some((key) => key !== 'mime' && key !== 'data') ||
    typeof image.mime !== 'string' ||
    typeof image.data !== 'string'
  )
    throw badRequest('images', 'encoding')
  if (!IMAGE_MIMES.some((mime) => mime === image.mime)) throw badRequest('images', 'mime')
  const data = image.data
  if (data.length > Math.ceil(MAX_IMAGE_BYTES / 3) * 4) throw badRequest('images', 'size', MAX_IMAGE_BYTES)
  if (data.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/u.test(data)) throw badRequest('images', 'encoding')
  let binary: string
  try {
    binary = atob(data)
  } catch {
    throw badRequest('images', 'encoding')
  }
  // atob accepts noncanonical padding bits; standard base64 does not.
  if (btoa(binary) !== data) throw badRequest('images', 'encoding')
  if (binary.length > MAX_IMAGE_BYTES) throw badRequest('images', 'size', MAX_IMAGE_BYTES)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export function encodeImage(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x8000)
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
  return btoa(binary)
}

function imageMime(bytes: Uint8Array): string {
  if (bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47) return 'image/png'
  if (bytes[0] === 0xff && bytes[1] === 0xd8) return 'image/jpeg'
  const signature = (start: number, end: number): string => String.fromCharCode(...bytes.subarray(start, end))
  if (signature(0, 4) === 'RIFF' && signature(8, 12) === 'WEBP') return 'image/webp'
  if (signature(0, 6) === 'GIF87a' || signature(0, 6) === 'GIF89a') return 'image/gif'
  throw badRequest('images', 'mime')
}

function interrupted(signal: AbortSignal): KitFailure {
  return signal.reason instanceof DOMException && signal.reason.name === 'TimeoutError'
    ? new KitFailure('timeout', 'Image request deadline exceeded')
    : new KitFailure('cancelled', 'Image request cancelled')
}

function abortable<T>(pending: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(interrupted(signal))
  const { promise, resolve, reject } = Promise.withResolvers<T>()
  const abort = (): void => {
    reject(interrupted(signal))
  }
  signal.addEventListener('abort', abort, { once: true })
  pending.then(resolve, reject)
  void promise.finally(() => signal.removeEventListener('abort', abort)).catch(() => {})
  return promise
}

async function bounded(response: Response, limit: number, signal: AbortSignal): Promise<Uint8Array> {
  const length = response.headers.get('content-length')
  if (length !== null && (!/^\d+$/u.test(length) || Number(length) > limit)) {
    void response.body?.cancel().catch(() => {})
    throw badRequest('images', 'size', limit)
  }
  if (!response.body) return new Uint8Array()
  const reader = response.body.getReader()
  const chunks: Uint8Array[] = []
  let size = 0
  let complete = false
  try {
    while (true) {
      const chunk = await abortable(reader.read(), signal)
      if (chunk.done) {
        complete = true
        break
      }
      if (chunk.value.length > limit - size) throw badRequest('images', 'size', limit)
      chunks.push(chunk.value)
      size += chunk.value.length
    }
  } finally {
    if (!complete) void reader.cancel().catch(() => {})
    reader.releaseLock()
  }
  const bytes = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.length
  }
  return bytes
}

function endpoint(base: string, edit: boolean): string {
  let url: URL
  try {
    url = new URL(`${base.replace(/\/+$/u, '')}/images/${edit ? 'edits' : 'generations'}`)
  } catch {
    throw badRequest('url', 'invalid')
  }
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.search || url.hash)
    throw badRequest('url', 'invalid')
  return url.href
}

function downloadTarget(value: string): string {
  let url: URL
  try {
    url = new URL(value)
  } catch {
    throw badRequest('url', 'invalid')
  }
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.hash)
    throw badRequest('url', 'invalid')
  return url.href
}

/**
 * One independent generation/edit. OutboundHttp MUST enforce host address policy
 * for BOTH endpoint and returned download URLs, including DNS, and reject redirects.
 * No global fetch, chat state, persistence, or retries after transport uncertainty.
 */
export async function generateImage(
  http: OutboundHttp,
  connection: ImageConnection,
  input: ImageRequest,
  signal: AbortSignal,
): Promise<ImagePart> {
  let dispatch: ImageDispatch = 'not_dispatched'
  const controller = new AbortController()
  const cancel = (): void => controller.abort(signal.reason)
  signal.addEventListener('abort', cancel, { once: true })
  if (signal.aborted) cancel()
  const timer = setTimeout(
    () => controller.abort(new DOMException('Image request deadline exceeded', 'TimeoutError')),
    IMAGE_TIMEOUT_MS,
  )
  const active = controller.signal
  try {
    if (active.aborted) throw interrupted(active)
    const prompt = input.prompt.trim()
    if (!prompt || [...prompt].length > 4000) throw badRequest('prompt', 'invalid')
    if (!IMAGE_SIZES.some((size) => size === input.size)) throw badRequest('size', 'invalid')
    if (input.images.length > MAX_EDIT_IMAGES) throw badRequest('images', 'count', MAX_EDIT_IMAGES)
    const edit = input.images.length > 0
    if (edit && !connection.canVision) throw new KitFailure('unsupported', 'Image editing unsupported')
    const target = endpoint((edit ? connection.imageEditBaseUrl : connection.imageBaseUrl) ?? connection.baseUrl, edit)
    let payload: BodyInit
    if (edit) {
      const form = new FormData()
      form.set('model', connection.modelId)
      form.set('prompt', prompt)
      form.set('size', input.size)
      form.set('n', '1')
      for (const [index, raw] of input.images.entries()) {
        const image = raw
        const bytes = decodeImage(image)
        const extension = image.mime === 'image/jpeg' ? 'jpg' : image.mime.slice(6)
        form.append(
          input.images.length === 1 ? 'image' : 'image[]',
          new Blob([bytes], { type: image.mime }),
          `image-${index + 1}.${extension}`,
        )
      }
      payload = form
    } else {
      payload = JSON.stringify({ model: connection.modelId, prompt, size: input.size, n: 1 })
    }
    const baseHeaders = new Headers(connection.headers)
    // Never allow a custom JSON header to break the platform multipart boundary.
    if (edit) baseHeaders.delete('content-type')
    else baseHeaders.set('content-type', 'application/json')
    const attempts = Math.max(1, connection.keys.length)
    for (let attempt = 0; attempt < attempts; attempt++) {
      if (active.aborted) throw interrupted(active)
      const headers = new Headers(baseHeaders)
      const key = connection.keys[attempt]
      if (key !== undefined) headers.set('authorization', `Bearer ${key}`)
      dispatch = 'unknown'
      const response = await abortable(
        http.fetch(target, { method: 'POST', headers, body: payload, redirect: 'manual', signal: active }),
        active,
      )
      if (response.redirected || (response.status >= 300 && response.status < 400)) {
        void response.body?.cancel().catch(() => {})
        throw new KitFailure('upstream', 'Image redirect refused', { status: response.status })
      }
      if (!response.ok) dispatch = 'rejected'
      const bytes = await bounded(response, response.ok ? RESPONSE_BYTES : 8192, active)
      if (!response.ok) {
        if ([401, 403, 429].includes(response.status) && attempt + 1 < attempts) continue
        // Deliberately never surface provider bodies: they can contain credentials.
        throw new KitFailure('upstream', 'Image provider rejected request', { status: response.status })
      }
      let body: unknown
      try {
        body = JSON.parse(new TextDecoder().decode(bytes))
      } catch {
        throw new KitFailure('upstream', 'Invalid image response')
      }
      const first: unknown = record(body) && Array.isArray(body.data) ? body.data[0] : undefined
      if (!record(first)) throw new KitFailure('upstream', 'No image in response')
      let imageBytes: Uint8Array
      if (typeof first.b64_json === 'string') {
        imageBytes = decodeImage({ mime: 'image/png', data: first.b64_json })
      } else if (typeof first.url === 'string') {
        const image = await abortable(
          http.fetch(downloadTarget(first.url), { method: 'GET', redirect: 'manual', signal: active }),
          active,
        )
        if (image.redirected || !image.ok) {
          void image.body?.cancel().catch(() => {})
          throw new KitFailure('upstream', 'Image download failed', { status: image.status })
        }
        const mime = image.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase()
        if (mime !== undefined && !IMAGE_MIMES.some((allowed) => allowed === mime)) {
          void image.body?.cancel().catch(() => {})
          throw badRequest('images', 'mime')
        }
        imageBytes = await bounded(image, MAX_IMAGE_BYTES, active)
        if (mime !== undefined && imageMime(imageBytes) !== mime) throw badRequest('images', 'mime')
      } else throw new KitFailure('upstream', 'No image in response')
      return { mime: imageMime(imageBytes), data: encodeImage(imageBytes) }
    }
    throw new KitFailure('internal', 'Image request has no attempts')
  } catch (error: unknown) {
    const failure = active.aborted
      ? interrupted(active)
      : error instanceof KitFailure
        ? error
        : new KitFailure('network', 'Image provider unreachable')
    throw new ImageFailure(failure.code, failure.message, dispatch, failure.details)
  } finally {
    clearTimeout(timer)
    signal.removeEventListener('abort', cancel)
  }
}
