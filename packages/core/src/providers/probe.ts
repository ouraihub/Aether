import type {
  ProbeError,
  ProbeErrorKind,
  ProbeResult,
  ProbedModel,
} from '@ouraihub/aether-contracts'
import type { OutboundHttp } from '../ports.js'
import { cancelled, excerpt, KitFailure } from '../error.js'
import {
  apiBase,
  authHeaders,
  prepare,
  type EndpointSnapshot,
} from './connection.js'
import { inferModel, inferReasoning } from './infer.js'

function at(value: unknown, name: string): unknown {
  return value !== null && typeof value === 'object' && name in value
    ? Reflect.get(value, name)
    : undefined
}
async function readBody(
  response: Response,
  limit: number,
): Promise<string | null> {
  if (Number(response.headers.get('content-length') ?? 0) > limit) {
    await response.body?.cancel()
    return null
  }
  const reader = response.body?.getReader()
  if (!reader) return ''
  const decoder = new TextDecoder()
  let size = 0
  let text = ''
  try {
    for (;;) {
      const item = await reader.read()
      if (item.done) return text + decoder.decode()
      size += item.value.byteLength
      if (size > limit) {
        await reader.cancel()
        return null
      }
      text += decoder.decode(item.value, { stream: true })
    }
  } finally {
    reader.releaseLock()
  }
}
export interface ProbeInput {
  endpoint: EndpointSnapshot
  signal: AbortSignal
}
/** Explicit bounded probe; key order is fixed by the caller, never rotated here. */
export async function probe(
  outbound: OutboundHttp,
  input: ProbeInput,
): Promise<ProbeResult> {
  const connection = prepare({ endpoint: input.endpoint, model: null })
  const probeUrl = `${connection.baseUrl}/models`
  const done = (
    models: ProbedModel[],
    error: ProbeError | null,
    suggestedProtocol: ProbeResult['suggestedProtocol'] = null,
  ): ProbeResult => ({ probeUrl, models, error, suggestedProtocol })
  const failure = (kind: ProbeErrorKind): ProbeResult =>
    done([], {
      kind,
      status: null,
      message: null,
      hint:
        kind === 'connect' && /^\[.*\]$|^\d+\.\d+\.\d+\.\d+$/.test(hostname)
          ? 'providers.hint.check_network'
          : null,
    })
  let hostname: string
  try {
    hostname = new URL(probeUrl).hostname
  } catch {
    throw new KitFailure('bad_request', 'Invalid probe URL', {
      field: 'baseUrl',
      reason: 'invalid_url',
    })
  }
  if (
    !connection.keys.length &&
    !['localhost', '127.0.0.1', '[::1]'].includes(hostname)
  )
    return failure('missing_key')
  let timeout = AbortSignal.timeout(20_000)
  const keys = connection.keys.length ? connection.keys : [null]
  let used: string | null = null
  let text: string | null = null
  try {
    for (let index = 0; index < keys.length; index++) {
      if (input.signal.aborted) throw cancelled()
      timeout = AbortSignal.timeout(20_000)
      const signal = AbortSignal.any([input.signal, timeout])
      used = keys[index] ?? null
      const response = await outbound.fetch(probeUrl, {
        headers: authHeaders(connection.protocol, used, connection.headers),
        redirect: 'manual',
        signal,
      })
      text = await readBody(response, response.ok ? 4 * 1024 * 1024 : 8192)
      if (response.ok) break
      if ([401, 403, 429].includes(response.status) && index + 1 < keys.length)
        continue
      const kind: ProbeErrorKind =
        response.status === 401 || response.status === 403
          ? 'unauthorized'
          : response.status === 404
            ? 'not_found'
            : response.status === 429
              ? 'rate_limited'
              : 'status'
      return done([], {
        kind,
        status: response.status,
        message: text ? excerpt(text, connection.keys) : null,
        hint:
          kind === 'unauthorized'
            ? 'providers.hint.check_key'
            : kind === 'not_found' && !input.endpoint.baseUrl.includes('/v1')
              ? 'providers.hint.missing_v1'
              : null,
      })
    }
    if (text === null) return failure('invalid_body')
    let body: unknown
    try {
      body = JSON.parse(text)
    } catch {
      return failure('invalid_body')
    }
    const items: unknown = Array.isArray(body)
      ? body
      : (at(body, 'data') ?? at(body, 'models'))
    if (!Array.isArray(items) || !items.length) return failure('invalid_body')
    const ids = new Set<string>()
    for (const item of items) {
      const id: unknown =
        typeof item === 'string' ? item : (at(item, 'id') ?? at(item, 'name'))
      if (typeof id !== 'string' || !id.trim()) return failure('invalid_body')
      ids.add(id)
    }
    const models = [...ids].sort().map((id): ProbedModel => {
      const inferred = inferModel(id)
      return {
        id,
        modelType: inferred.modelType,
        typeSource: 'inferred',
        contextWindow: inferred.contextWindow,
        maxOutput: inferred.maxOutput,
        canVision: inferred.canVision,
        supportsTools: inferred.supportsTools,
        reasoning:
          inferred.modelType === 'chat'
            ? inferReasoning(
                id,
                connection.protocol,
                input.endpoint.baseUrl,
                inferred.maxOutput,
              )
            : null,
        nativeWebSearch: false,
        name: null,
        groupName: inferred.group,
        pricing: null,
        group: inferred.group,
      }
    })
    const claude = models.find((model) => model.group === 'claude')
    if (claude && connection.protocol !== 'anthropic-messages') {
      const headers = authHeaders(
        'anthropic-messages',
        used,
        connection.headers,
      )
      headers.set('content-type', 'application/json')
      try {
        const signal = AbortSignal.any([
          input.signal,
          AbortSignal.timeout(20_000),
        ])
        const suggestion = await outbound.fetch(
          `${apiBase(input.endpoint.baseUrl)}/messages`,
          {
            method: 'POST',
            headers,
            body: JSON.stringify({
              model: claude.id,
              max_tokens: 1,
              messages: [],
            }),
            redirect: 'manual',
            signal,
          },
        )
        await suggestion.body?.cancel()
        if ([400, 422].includes(suggestion.status))
          return done(models, null, 'anthropic-messages')
      } catch {
        if (input.signal.aborted) throw cancelled()
      }
    }
    return done(models, null)
  } catch (error) {
    if (input.signal.aborted) throw cancelled()
    if (timeout.aborted) return failure('timeout')
    if (
      error instanceof KitFailure &&
      error.details?.reason === 'blocked_address'
    )
      return failure('blocked_address')
    return failure('connect')
  }
}
