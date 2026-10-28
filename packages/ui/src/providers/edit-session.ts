import { KitClientError, type CallOptions, type ProbeResult, type Provider, type ProvidersPort } from '../ports'
import { adopt, probeTarget, savable, toSave, type ProviderDraft } from './draft'

export interface ProviderProbeBatch {
  results: { keyId: string | null; result: ProbeResult }[]
  usable: ProbeResult | null
}
export interface ProviderEditSession {
  commit(): Promise<void>
  probe(endpointSlug: string, options?: CallOptions): Promise<ProviderProbeBatch>
  invalidate(): void
}

const cancelled = () =>
  new KitClientError({ code: 'cancelled', message: 'Provider editing session cancelled', details: {} })
const identityError = () =>
  new KitClientError({ code: 'internal', message: 'Saved provider identity is missing or ambiguous', details: {} })
// Copy plain DTO data without losing invalid numeric edits or retaining Svelte proxies.
function snapshot<T>(value: T): T {
  if (Array.isArray(value)) return value.map((item) => snapshot(item)) as T
  if (value !== null && typeof value === 'object')
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, snapshot(item)])) as T
  return value
}
export function createProviderEditSession(
  port: ProvidersPort,
  state: {
    readDraft(): ProviderDraft
    writeDraft(next: ProviderDraft): void
    readProviders(): readonly Provider[]
    onSaved(next: Provider[]): void
  },
): ProviderEditSession {
  let valid = true
  let saving = false
  let dirty = false
  let waiters: { resolve(): void; reject(error: unknown): void }[] = []
  let activeProbe: { cancel(): void } | null = null

  function settle(error?: unknown) {
    const pending = waiters
    waiters = []
    for (const waiter of pending) {
      if (error !== undefined) waiter.reject(error)
      else waiter.resolve()
    }
  }
  async function drain() {
    try {
      while (valid && dirty) {
        dirty = false
        const sent = snapshot(savable(state.readDraft()))
        const before = new Set(state.readProviders().map((provider) => provider.id))
        const next = await port.save(toSave(sent))
        if (!valid) return
        const matches = next.filter((provider) =>
          sent.id === null ? !before.has(provider.id) : provider.id === sent.id,
        )
        const stored = matches[0]
        if (matches.length !== 1 || !stored) throw identityError()
        for (const endpoint of sent.endpoints) {
          const records = stored.endpoints.filter((record) =>
            endpoint.saved ? record.id === endpoint.saved.id : record.slug === endpoint.slug,
          )
          if (records.length !== 1) throw identityError()
        }
        state.writeDraft(adopt(state.readDraft(), sent, stored))
        if (!valid) return
        state.onSaved(next)
      }
      if (valid) settle()
    } catch (error) {
      if (valid) {
        dirty = true
        settle(error)
      }
    } finally {
      saving = false
    }
  }
  function commit(): Promise<void> {
    if (!valid) return Promise.reject(cancelled())
    dirty = true
    const pending = Promise.withResolvers<void>()
    waiters.push(pending)
    if (!saving) {
      saving = true
      void drain()
    }
    return pending.promise
  }
  function probe(slug: string, options?: CallOptions): Promise<ProviderProbeBatch> {
    activeProbe?.cancel()
    if (!valid || options?.signal?.aborted) return Promise.reject(cancelled())
    const controller = new AbortController()
    let stopped = false
    const cancellation = Promise.withResolvers<never>()
    const request = {
      cancel() {
        if (stopped) return
        stopped = true
        controller.abort()
        cancellation.reject(cancelled())
      },
    }
    activeProbe = request
    const ensureCurrent = () => {
      if (!valid || stopped || options?.signal?.aborted) throw cancelled()
    }
    options?.signal?.addEventListener('abort', request.cancel, { once: true })
    const run = async (): Promise<ProviderProbeBatch> => {
      await commit()
      ensureCurrent()
      const endpoint = snapshot(state.readDraft().endpoints.find((item) => item.slug === slug) ?? null)
      if (!endpoint)
        throw new KitClientError({ code: 'not_found', message: 'Provider endpoint no longer exists', details: {} })
      const enabled = endpoint.saved?.keys.filter((key) => key.enabled) ?? []
      const keyIds = enabled.length >= 2 ? enabled.map((key) => key.id) : [null]
      const batch: ProviderProbeBatch = { results: [], usable: null }
      for (const keyId of keyIds) {
        ensureCurrent()
        const result = await port.probe(probeTarget(endpoint, keyId ?? undefined), { signal: controller.signal })
        ensureCurrent()
        batch.results.push({ keyId, result })
        if (result.error === null) batch.usable ??= result
      }
      return batch
    }
    return Promise.race([run(), cancellation.promise]).finally(() => {
      options?.signal?.removeEventListener('abort', request.cancel)
      if (activeProbe === request) activeProbe = null
    })
  }
  return {
    commit,
    probe,
    invalidate() {
      if (!valid) return
      valid = false
      dirty = false
      activeProbe?.cancel()
      activeProbe = null
      settle(cancelled())
    },
  }
}
