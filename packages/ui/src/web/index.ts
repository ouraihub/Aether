// Browser HTTP adapter. Only enabled capabilities are exposed; desktop/IPC never enter this module.
import { isContract, validateRunAccepted, validateRunEvent, validateRunSnapshot } from '@ouraihub/aether-contracts'
import type { ContractMap, ContractName, KitError, RunAccepted, RunEvent } from '@ouraihub/aether-contracts'
import { KitClientError } from '../ports/contract'
import type { CallOptions, KitPorts } from '../ports/contract'

export interface WebPortsOptions {
  baseURL: string
  fetch: (input: string, init: RequestInit) => Promise<Response>
  csrf: string | (() => string | Promise<string>)
  credentials?: RequestCredentials
  chat?: boolean
  conversations?: boolean
  providers?: boolean
  search?: boolean
  skills?: boolean
  mcpClient?: boolean
  /** A disconnected stream is not a run outcome. Host may display pending confirmation. */
  onStreamError?: (runId: string, error: KitClientError) => void
}

type Guard<T> = (value: unknown) => value is T
const contract =
  <N extends ContractName>(name: N): Guard<ContractMap[N]> =>
  (value): value is ContractMap[N] =>
    isContract(name, value)
const array =
  <T>(guard: Guard<T>): Guard<T[]> =>
  (value): value is T[] =>
    Array.isArray(value) && value.every((item: unknown) => guard(item))
const string: Guard<string> = (value): value is string => typeof value === 'string'
const count: Guard<{ count: number }> = (value): value is { count: number } =>
  typeof value === 'object' &&
  value !== null &&
  'count' in value &&
  typeof value.count === 'number' &&
  Number.isSafeInteger(value.count) &&
  value.count >= 0 &&
  Object.keys(value).length === 1
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)
const failure = (code: KitError['code'], message: string, reason?: string): KitClientError =>
  new KitClientError({ code, message, ...(reason ? { details: { reason } } : {}) })
const invalidResponse = () => failure('upstream', 'Invalid server response', 'invalid_response')
const normalize = (error: unknown, signal?: AbortSignal): KitClientError => {
  if (error instanceof KitClientError) return error
  return signal?.aborted ? failure('cancelled', 'Request cancelled') : failure('network', 'Unable to contact server')
}
const id = encodeURIComponent

export function createWebPorts(options: WebPortsOptions): KitPorts {
  const base = new URL(options.baseURL)
  if (!['http:', 'https:'].includes(base.protocol) || base.username || base.password || base.search || base.hash)
    throw failure('bad_request', 'Invalid API base URL')
  const api = new URL('/api/v1/', base)
  const request = async (path: string, method: string, body?: unknown, signal?: AbortSignal): Promise<Response> => {
    if (signal?.aborted) throw failure('cancelled', 'Request cancelled')
    try {
      const headers = new Headers({ Accept: path.endsWith('/events') ? 'text/event-stream' : 'application/json' })
      if (method !== 'GET') {
        const csrf = typeof options.csrf === 'function' ? await options.csrf() : options.csrf
        headers.set('X-CSRF-Token', csrf)
      }
      if (body !== undefined) headers.set('Content-Type', 'application/json')
      if (signal?.aborted) throw failure('cancelled', 'Request cancelled')
      const response = await options.fetch(new URL(path, api).href, {
        method,
        headers,
        credentials: options.credentials ?? 'include',
        redirect: 'error',
        ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
        ...(signal ? { signal } : {}),
      })
      if (!response.ok) {
        let value: unknown
        try {
          value = await response.json()
        } catch {
          throw invalidResponse()
        }
        if (isContract('KitError', value)) throw new KitClientError(value)
        if (record(value) && isContract('KitError', value.error)) throw new KitClientError(value.error)
        throw invalidResponse()
      }
      return response
    } catch (error) {
      throw normalize(error, signal)
    }
  }
  const json = async <T>(
    path: string,
    method: string,
    guard: Guard<T>,
    body?: unknown,
    call?: CallOptions,
  ): Promise<T> => {
    const response = await request(path, method, body, call?.signal)
    let value: unknown
    try {
      value = await response.json()
    } catch {
      throw invalidResponse()
    }
    if (!guard(value)) throw invalidResponse()
    return value
  }
  const ack = async (path: string, method = 'POST', body: unknown = {}) => {
    await json(path, method, contract('Ack'), body)
  }
  const ports: KitPorts = {}
  if (options.chat) {
    const cancel = (runId: string) => ack(`runs/${id(runId)}/cancel`)
    ports.chat = {
      cancel,
      approve: (runId, callId, allow) =>
        ack(`runs/${id(runId)}/approval`, 'POST', { callId, decision: allow ? 'allow' : 'deny' }),
      getSnapshot: (runId) => json(`runs/${id(runId)}`, 'GET', validateRunSnapshot),
      start: async (input, onEvent, call) => {
        const signal = call?.signal
        if (signal?.aborted) throw failure('cancelled', 'Request cancelled')
        let accepted: RunAccepted | undefined
        let ended = false
        let requested = false
        let dispatched = false
        const cleanup = () => signal?.removeEventListener('abort', abort)
        const dispatchCancel = () => {
          if (!accepted || ended || dispatched) return
          dispatched = true
          const runId = accepted.runId
          void cancel(runId).catch((error: unknown) => options.onStreamError?.(runId, normalize(error)))
        }
        const abort = () => {
          requested = true
          dispatchCancel()
        }
        const emit = (event: RunEvent) => {
          if (ended) return
          if (event.kind === 'end') {
            ended = true
            cleanup()
          }
          onEvent(event)
        }
        signal?.addEventListener('abort', abort, { once: true })
        try {
          // Keep the acceptance response readable after abort: its run ID is needed to cancel the reservation.
          const response = await request('runs', 'POST', input)
          if (response.status !== 202) throw invalidResponse()
          let envelope: unknown
          try {
            envelope = await response.json()
          } catch {
            throw invalidResponse()
          }
          if (
            !record(envelope) ||
            Object.keys(envelope).some((key) => key !== 'accepted' && key !== 'events') ||
            !validateRunAccepted(envelope.accepted) ||
            typeof envelope.events !== 'string'
          )
            throw invalidResponse()
          accepted = envelope.accepted
          const events = new URL(envelope.events, base)
          if (
            events.origin !== base.origin ||
            events.pathname !== `/api/v1/runs/${id(accepted.runId)}/events` ||
            events.search ||
            events.hash ||
            events.username ||
            events.password
          )
            throw invalidResponse()
          const runId = accepted.runId
          // The Promise acknowledges acceptance only; the subscriber delivers all subsequent outcomes.
          void (async () => {
            try {
              if (requested || signal?.aborted) {
                dispatched = true
                await cancel(runId).catch((error: unknown) => options.onStreamError?.(runId, normalize(error)))
              }
              const stream = await request(`runs/${id(runId)}/events`, 'GET')
              if (!stream.headers.get('Content-Type')?.toLowerCase().startsWith('text/event-stream') || !stream.body)
                throw invalidResponse()
              await consumeEvents(stream.body, emit, () => ended)
              if (!ended) throw failure('network', 'Event stream disconnected', 'stream_disconnected')
            } catch (error) {
              if (!ended) {
                dispatchCancel()
                try {
                  const snapshot = await json(`runs/${id(runId)}`, 'GET', validateRunSnapshot)
                  if (snapshot.status !== 'running')
                    emit({
                      kind: 'end',
                      status: snapshot.status,
                      ...(snapshot.error ? { error: snapshot.error } : {}),
                      ...(snapshot.exhausted !== undefined ? { exhausted: snapshot.exhausted } : {}),
                    })
                } catch {
                  /* A missing snapshot is unknown, never a fabricated completion. */
                }
                if (!ended) options.onStreamError?.(runId, normalize(error))
              }
            } finally {
              cleanup()
            }
          })()
          return accepted
        } catch (error) {
          if (accepted) dispatchCancel()
          ended = true
          cleanup()
          throw normalize(error)
        }
      },
    }
  }
  if (options.conversations) {
    const path = (conversationId: string) => `conversations/${id(conversationId)}`
    ports.conversations = {
      list: (scope) =>
        json(
          `conversations${scope === undefined ? '' : `?scope=${id(scope)}`}`,
          'GET',
          array(contract('ConversationSummary')),
        ),
      get: (conversationId) => json(path(conversationId), 'GET', contract('Conversation')),
      remove: (conversationId) => ack(path(conversationId), 'DELETE'),
      rename: (conversationId, title) =>
        json(`${path(conversationId)}/rename`, 'POST', contract('ConversationSummary'), { title }),
      editTurn: (conversationId, turnId, text, expectedText) =>
        json(`${path(conversationId)}/edit-turn`, 'POST', contract('Conversation'), { turnId, text, expectedText }),
      removeTurn: (conversationId, turnId) =>
        json(`${path(conversationId)}/remove-turn`, 'POST', contract('Conversation'), { turnId }),
      fork: (conversationId, turnId) =>
        json(`${path(conversationId)}/fork`, 'POST', contract('Conversation'), { turnId }),
      selectTurn: (conversationId, turnId) =>
        json(`${path(conversationId)}/select-turn`, 'POST', contract('Conversation'), { turnId }),
    }
  }
  if (options.providers) {
    const providers = array(contract('Provider'))
    ports.providers = {
      revealNeedsPassword: true,
      keyStrategies: false,
      list: () => json('providers', 'GET', providers),
      save: (input) => json('providers', 'POST', providers, input),
      remove: (providerId) => json(`providers/${id(providerId)}`, 'DELETE', providers),
      setEnabled: (providerId, enabled) => json(`providers/${id(providerId)}/enabled`, 'POST', providers, { enabled }),
      modelOptions: (filter = {}) => {
        const query = new URLSearchParams()
        if (filter.modelType !== undefined) query.set('modelType', filter.modelType)
        if (filter.supportsTools !== undefined) query.set('supportsTools', String(filter.supportsTools))
        return json(`providers/model-options?${query}`, 'GET', array(contract('ModelOption')))
      },
      probe: (target, call) => json('providers/probe', 'POST', contract('ProbeResult'), target, call),
      saveKey: (input) => json('providers/keys', 'POST', providers, input),
      removeKey: (endpointId, keyId) => json(`providers/${id(endpointId)}/keys/${id(keyId)}`, 'DELETE', providers),
      revealKey: (keyId, currentPassword) =>
        json(`providers/keys/${id(keyId)}/reveal`, 'POST', string, { currentPassword }),
    }
  }
  if (options.search) {
    const settings = contract('SearchSettingsDto')
    ports.search = {
      revealNeedsPassword: true,
      settings: (capability, call) =>
        json(`search/settings?capability=${id(capability)}`, 'GET', settings, undefined, call),
      save: (input, call) => json('search/settings', 'POST', settings, input, call),
      setDefault: (capability, provider, call) =>
        json('search/default', 'POST', settings, { capability, provider }, call),
      savePreferences: (input, call) => json('search/preferences', 'PUT', contract('SearchPreferences'), input, call),
      saveKey: (capability, provider, value, keyId, call) =>
        json('search/keys', 'POST', settings, { capability, provider, value, keyId }, call),
      removeKey: (capability, provider, keyId, call) =>
        json('search/keys', 'DELETE', settings, { capability, provider, keyId }, call),
      reveal: (capability, provider, keyId, currentPassword, call) =>
        json('search/keys/reveal', 'POST', string, { capability, provider, keyId, currentPassword }, call),
      check: (capability, provider, call) => json('search/check', 'POST', count, { capability, provider }, call),
      search: (query, call) => json('search/search', 'POST', contract('SearchResult'), { query }, call),
      fetch: (target, call) => json('search/fetch', 'POST', contract('SearchPage'), { url: target }, call),
    }
  }
  if (options.skills) {
    const skills = array(contract('SkillMeta'))
    ports.skills = {
      list: () => json('skills', 'GET', skills),
      get: (skillId) => json(`skills/${id(skillId)}`, 'GET', contract('SkillDto')),
      preview: (source) => json('skills/preview', 'POST', contract('Preview'), { source }),
      install: (source, confirmedDigest, replace) =>
        json('skills/install', 'POST', skills, { source, confirmedDigest, replace }),
      save: (input) => json('skills/save', 'POST', skills, input),
      duplicate: (skillId, newName) => json(`skills/${id(skillId)}/duplicate`, 'POST', skills, { newName }),
      reorder: (ids) => json('skills/reorder', 'POST', skills, { ids }),
      remove: (skillId) => json(`skills/${id(skillId)}`, 'DELETE', skills),
    }
  }
  if (options.mcpClient) {
    const servers = array(contract('ServerDto'))
    ports.mcpClient = {
      list: () => json('mcp/servers', 'GET', servers),
      save: (input) =>
        json(input.id ? `mcp/servers/${id(input.id)}` : 'mcp/servers', input.id ? 'PUT' : 'POST', servers, input),
      remove: (serverId) => json(`mcp/servers/${id(serverId)}`, 'DELETE', servers),
      refresh: (serverId) => json(`mcp/servers/${id(serverId)}/refresh`, 'POST', array(contract('ToolDto')), {}),
      setToolApproval: (serverId, tool, approval) =>
        ack(`mcp/servers/${id(serverId)}/tools/${id(tool)}/approval`, 'PUT', { approval }),
    }
  }
  return ports
}

async function consumeEvents(
  body: ReadableStream<Uint8Array>,
  emit: (event: RunEvent) => void,
  ended: () => boolean,
): Promise<void> {
  const reader = body.getReader()
  const decoder = new TextDecoder('utf-8', { fatal: true })
  let buffer = ''
  let data: string[] = []
  let eventName = ''
  let size = 0
  const line = (value: string) => {
    if (value === '') {
      if (data.length > 0) {
        if (eventName && eventName !== 'run_event') throw invalidResponse()
        let parsed: unknown
        try {
          parsed = JSON.parse(data.join('\n'))
        } catch {
          throw invalidResponse()
        }
        if (!validateRunEvent(parsed)) throw invalidResponse()
        emit(parsed)
      }
      data = []
      eventName = ''
      size = 0
      return
    }
    size += value.length
    if (size > 1024 * 1024) throw invalidResponse()
    if (value.startsWith(':')) return
    const separator = value.indexOf(':')
    const field = separator < 0 ? value : value.slice(0, separator)
    let content = separator < 0 ? '' : value.slice(separator + 1)
    if (content.startsWith(' ')) content = content.slice(1)
    if (field === 'data') data.push(content)
    else if (field === 'event') eventName = content
  }
  try {
    for (;;) {
      const chunk = await reader.read()
      buffer += decoder.decode(chunk.value, { stream: !chunk.done })
      let at = 0
      while (at < buffer.length) {
        const found = buffer.slice(at).search(/[\r\n]/)
        if (found < 0) break
        const end = at + found
        if (!chunk.done && buffer[end] === '\r' && end === buffer.length - 1) break
        line(buffer.slice(at, end))
        at = end + (buffer[end] === '\r' && buffer[end + 1] === '\n' ? 2 : 1)
        if (ended()) return
      }
      buffer = buffer.slice(at)
      if (buffer.length + size > 1024 * 1024) throw invalidResponse()
      if (chunk.done) return
    }
  } finally {
    try {
      await reader.cancel()
    } finally {
      reader.releaseLock()
    }
  }
}
