import type { ImagePart, KitErrorCode, Details, TokenUsage } from '@ouraihub/aether-contracts'
import { badRequest, KitFailure } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import type { Connection } from '../types.js'
import { emptyUsage } from '../pricing.js'
import { validateImageBytes } from './validation.js'
export { validateImageBytes } from './validation.js'

export const MAX_IMAGE_BYTES = 4 * 1024 * 1024
export const MAX_EDIT_IMAGES = 4
export const IMAGE_MIMES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'] as const
export const IMAGE_SIZES = ['1024x1024', '1024x1536', '1536x1024'] as const
export const IMAGE_TIMEOUT_MS = 120_000
export const MAX_GENERATED_IMAGE_BYTES = 10 * 1024 * 1024
export const MAX_IMAGE_INPUT_BYTES = 20 * 1024 * 1024
export type ImageProfile = 'generic' | 'gpt-image' | 'siliconflow'
export type ImageQuality = 'low' | 'medium' | 'high'
export interface ImageLimits {
  readonly maxImageBytes?: number
  readonly timeoutMs?: number
  readonly profile?: ImageProfile
}
export interface ImageGeneration {
  readonly image: ImagePart
  readonly revisedPrompt: string | null
  readonly usage: TokenUsage
  readonly totalTokens: number | null
}
export type ImageSize = (typeof IMAGE_SIZES)[number]
export interface ImageRequest {
  readonly prompt: string
  readonly size: ImageSize
  readonly images: readonly ImagePart[]
  readonly quality?: ImageQuality
  readonly mask?: ImagePart
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

export function decodeImage(image: ImagePart, maxBytes = MAX_IMAGE_BYTES): Uint8Array<ArrayBuffer> {
  if (
    !record(image) ||
    Object.keys(image).some((key) => key !== 'mime' && key !== 'data') ||
    typeof image.mime !== 'string' ||
    typeof image.data !== 'string'
  )
    throw badRequest('images', 'encoding')
  if (!IMAGE_MIMES.some((mime) => mime === image.mime)) throw badRequest('images', 'mime')
  const data = image.data
  if (data.length > Math.ceil(maxBytes / 3) * 4) throw badRequest('images', 'size', maxBytes)
  if (data.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/u.test(data)) throw badRequest('images', 'encoding')
  let binary: string
  try {
    binary = atob(data)
  } catch {
    throw badRequest('images', 'encoding')
  }
  // atob accepts noncanonical padding bits; standard base64 does not.
  if (btoa(binary) !== data) throw badRequest('images', 'encoding')
  if (binary.length > maxBytes) throw badRequest('images', 'size', maxBytes)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export function encodeImage(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x8000)
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
  return btoa(binary)
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

/** Only known profiles enable provider-specific options; callers may explicitly select one. */
export function imageCapabilities(connection: ImageConnection, limits: ImageLimits = {}) {
  const host = new URL(endpoint(connection.imageBaseUrl ?? connection.baseUrl, false)).hostname
  const profile =
    limits.profile ??
    (['api.siliconflow.cn', 'api.siliconflow.com'].includes(host)
      ? 'siliconflow'
      : ['gpt-image-1', 'gpt-image-1-mini', 'gpt-image-1.5', 'gpt-image-2'].includes(connection.modelId)
        ? 'gpt-image'
        : 'generic')
  if (!['generic', 'gpt-image', 'siliconflow'].includes(profile)) throw badRequest('profile', 'invalid')
  return {
    profile,
    edit: connection.canVision && profile !== 'siliconflow',
    quality: profile === 'gpt-image',
    mask: profile === 'gpt-image',
  }
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
  limits: ImageLimits = {},
): Promise<ImageGeneration> {
  let dispatch: ImageDispatch = 'not_dispatched'
  const controller = new AbortController()
  const cancel = (): void => controller.abort(signal.reason)
  signal.addEventListener('abort', cancel, { once: true })
  if (signal.aborted) cancel()
  const timeoutMs = limits.timeoutMs ?? IMAGE_TIMEOUT_MS
  const maxImageBytes = limits.maxImageBytes ?? MAX_IMAGE_BYTES
  const validLimits =
    Number.isSafeInteger(timeoutMs) &&
    timeoutMs > 0 &&
    timeoutMs <= 180_000 &&
    Number.isSafeInteger(maxImageBytes) &&
    maxImageBytes > 0 &&
    maxImageBytes <= MAX_GENERATED_IMAGE_BYTES
  const timer = validLimits
    ? setTimeout(() => controller.abort(new DOMException('Image request deadline exceeded', 'TimeoutError')), timeoutMs)
    : undefined
  const active = controller.signal
  try {
    if (active.aborted) throw interrupted(active)
    if (!validLimits) throw badRequest('limits', 'invalid')
    const prompt = input.prompt.trim()
    if (!prompt || [...prompt].length > 4000) throw badRequest('prompt', 'invalid')
    if (!IMAGE_SIZES.some((size) => size === input.size)) throw badRequest('size', 'invalid')
    if (input.images.length > MAX_EDIT_IMAGES) throw badRequest('images', 'count', MAX_EDIT_IMAGES)
    const edit = input.images.length > 0
    const capabilities = imageCapabilities(connection, limits)
    if (edit && !capabilities.edit) throw new KitFailure('unsupported', 'Image editing unsupported')
    if (input.quality !== undefined) {
      if (!['low', 'medium', 'high'].includes(input.quality)) throw badRequest('quality', 'invalid')
      if (!capabilities.quality) throw new KitFailure('unsupported', 'Image quality unsupported')
    }
    if (input.mask !== undefined && (!edit || !capabilities.mask))
      throw new KitFailure('unsupported', 'Image mask unsupported')
    let inputBytes = 0
    const decoded = input.images.map((image) => {
      const bytes = decodeImage(image, maxImageBytes)
      validateImageBytes(bytes, image.mime, maxImageBytes)
      inputBytes += bytes.length
      return bytes
    })
    const maskBytes = input.mask === undefined ? undefined : decodeImage(input.mask, maxImageBytes)
    if (maskBytes && input.mask) {
      const mask = validateImageBytes(maskBytes, 'image/png', maxImageBytes)
      const firstBytes = decoded[0]
      if (firstBytes === undefined) throw badRequest('mask', 'requires_image')
      const first = validateImageBytes(firstBytes, undefined, maxImageBytes)
      if (input.mask.mime !== 'image/png' || !mask.alpha || mask.width !== first.width || mask.height !== first.height)
        throw badRequest('mask', 'invalid')
      inputBytes += maskBytes.length
    }
    if (inputBytes > MAX_IMAGE_INPUT_BYTES) throw badRequest('images', 'size', MAX_IMAGE_INPUT_BYTES)
    const target = endpoint((edit ? connection.imageEditBaseUrl : connection.imageBaseUrl) ?? connection.baseUrl, edit)
    let payload: BodyInit
    if (edit) {
      const form = new FormData()
      form.set('model', connection.modelId)
      form.set('prompt', prompt)
      form.set('size', input.size)
      form.set('n', '1')
      if (capabilities.profile === 'gpt-image') {
        form.set('output_format', 'png')
        form.set('background', 'opaque')
      }
      if (input.quality !== undefined) form.set('quality', input.quality)
      if (maskBytes) form.set('mask', new Blob([maskBytes], { type: 'image/png' }), 'mask.png')
      for (const [index, raw] of input.images.entries()) {
        const image = raw
        const bytes = decoded[index]
        if (bytes === undefined) throw badRequest('images', 'encoding')
        const extension = image.mime === 'image/jpeg' ? 'jpg' : image.mime.slice(6)
        form.append(
          input.images.length === 1 ? 'image' : 'image[]',
          new Blob([bytes], { type: image.mime }),
          `image-${index + 1}.${extension}`,
        )
      }
      payload = form
    } else {
      payload = JSON.stringify(
        capabilities.profile === 'siliconflow'
          ? { model: connection.modelId, prompt, image_size: input.size }
          : {
              model: connection.modelId,
              prompt,
              size: input.size,
              n: 1,
              ...(capabilities.profile === 'gpt-image' ? { output_format: 'png', background: 'opaque' } : {}),
              ...(input.quality === undefined ? {} : { quality: input.quality }),
            },
      )
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
      if (response.status >= 400 && response.status < 500) dispatch = 'rejected'
      const bytes = await bounded(response, response.ok ? Math.ceil(maxImageBytes / 3) * 4 + 64 * 1024 : 8192, active)
      if (!response.ok) {
        if ([401, 403, 429].includes(response.status) && attempt + 1 < attempts) continue
        // Deliberately never surface provider bodies: they can contain credentials.
        const code =
          response.status === 401 || response.status === 403
            ? 'unauthorized'
            : response.status === 429
              ? 'rate_limited'
              : 'upstream'
        throw new KitFailure(code, 'Image provider rejected request', { status: response.status })
      }
      let body: unknown
      try {
        body = JSON.parse(new TextDecoder().decode(bytes))
      } catch {
        throw new KitFailure('upstream', 'Invalid image response')
      }
      const images: unknown = record(body)
        ? capabilities.profile === 'siliconflow'
          ? body.images
          : body.data
        : undefined
      if (!Array.isArray(images) || images.length !== 1) throw new KitFailure('upstream', 'Expected exactly one image')
      const first: unknown = images[0]
      if (!record(first) || (typeof first.b64_json === 'string') === (typeof first.url === 'string'))
        throw new KitFailure('upstream', 'Invalid image response')
      let imageBytes: Uint8Array
      let expectedMime: string | undefined
      if (typeof first.b64_json === 'string') {
        imageBytes = decodeImage({ mime: 'image/png', data: first.b64_json }, maxImageBytes)
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
        if (mime && mime !== 'application/octet-stream' && !IMAGE_MIMES.some((allowed) => allowed === mime)) {
          void image.body?.cancel().catch(() => {})
          throw badRequest('images', 'mime')
        }
        imageBytes = await bounded(image, maxImageBytes, active)
        expectedMime = mime && mime !== 'application/octet-stream' ? mime : undefined
      } else throw new KitFailure('upstream', 'No image in response')
      const checked = validateImageBytes(imageBytes, expectedMime, maxImageBytes)
      const usage = emptyUsage()
      let totalTokens: number | null = null
      if (record(body) && record(body.usage)) {
        const count = (value: unknown): number | null =>
          typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null
        totalTokens = count(body.usage.total_tokens)
        usage.inputTokens = count(body.usage.input_tokens ?? body.usage.prompt_tokens)
        usage.outputTokens = count(body.usage.output_tokens ?? body.usage.completion_tokens)
        const inputDetails = body.usage.input_tokens_details ?? body.usage.prompt_tokens_details
        const outputDetails = body.usage.output_tokens_details ?? body.usage.completion_tokens_details
        if (record(inputDetails)) {
          usage.cacheReadTokens = count(inputDetails.cached_tokens)
          usage.cacheWriteTokens = count(inputDetails.cache_creation_tokens ?? body.usage.cache_creation_input_tokens)
        }
        if (!record(inputDetails)) usage.cacheWriteTokens = count(body.usage.cache_creation_input_tokens)
        if (record(outputDetails)) usage.reasoningTokens = count(outputDetails.reasoning_tokens)
      }
      return {
        image: { mime: checked.mime, data: encodeImage(imageBytes) },
        revisedPrompt: typeof first.revised_prompt === 'string' ? first.revised_prompt : null,
        usage,
        totalTokens,
      }
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
    clearTimeout(timer ?? 0)
    signal.removeEventListener('abort', cancel)
  }
}
