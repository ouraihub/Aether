import { describe, expect, it, vi } from 'vitest'
import {
  createFakePorts,
  KitClientError,
  type Endpoint,
  type ProbeResult,
  type ProbeTarget,
  type Provider,
} from '../ports'
import { mergeModels, newEndpoint, toDraft, type ProviderDraft } from './draft'
import { createProviderEditSession } from './edit-session'

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected provider fixture is missing')
  return value
}

const endpoint: Endpoint = {
  id: 'endpoint',
  slug: 'main',
  baseUrl: 'https://provider.example',
  protocol: 'openai-chat',
  keyStrategy: 'round_robin',
  imageBaseUrl: null,
  imageEditBaseUrl: null,
  headers: {},
  models: [],
  probeUrl: 'https://provider.example/v1/models',
  keys: ['first', 'second'].map((id) => ({
    id,
    label: '',
    enabled: true,
    masked: 'masked',
    readable: true,
    exhaustedUntil: null,
    exhaustedReason: null,
  })),
}
const stored: Provider = {
  id: 'provider',
  name: 'Original',
  enabled: true,
  notes: '',
  presetId: null,
  endpoints: [endpoint],
}
const result: ProbeResult = { probeUrl: endpoint.probeUrl, models: [], error: null, suggestedProtocol: null }

function setup(
  overrides: NonNullable<Parameters<typeof createFakePorts>[0]>['providers'] = {},
  initial = toDraft(stored),
) {
  let draft = initial
  let providers = [stored]
  const onSaved = vi.fn((next: Provider[]) => {
    providers = next
  })
  const port = required(
    createFakePorts({
      providers: { save: async () => [stored], probe: async () => result, ...overrides },
    }).providers,
  )
  const session = createProviderEditSession(port, {
    readDraft: () => draft,
    writeDraft: (next) => {
      draft = next
    },
    readProviders: () => providers,
    onSaved,
  })
  return { session, onSaved, read: () => draft }
}

// No timers or transport mocks: deferred port responses exercise the actual workflow boundaries.
describe('provider edit session', () => {
  it('coalesces saves, holds every waiter to queue drain and adopts only the immutable sent identities', async () => {
    const first = Promise.withResolvers<Provider[]>()
    const second = Promise.withResolvers<Provider[]>()
    const save = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const initial: ProviderDraft = { ...toDraft(stored), id: null, endpoints: [newEndpoint([])] }
    const flow = setup({ save }, initial)
    const completed = vi.fn()
    const one = flow.session.commit().then(completed)
    initial.name = 'Edited while saving'
    required(initial.endpoints[0]).baseUrl = 'https://new.example'
    const two = flow.session.commit().then(completed)
    const created = { ...stored, id: 'created', endpoints: [{ ...endpoint, id: 'created-endpoint', slug: 'ep-1' }] }
    first.resolve([stored, created])
    await first.promise
    await Promise.resolve()
    expect(save).toHaveBeenCalledTimes(2)
    expect(completed).not.toHaveBeenCalled()
    expect(save.mock.calls[0]?.[0]).toMatchObject({ name: 'Original', endpoints: [{ baseUrl: '' }] })
    expect(save.mock.calls[1]?.[0]).toMatchObject({
      id: 'created',
      name: 'Edited while saving',
      endpoints: [{ id: 'created-endpoint', baseUrl: 'https://new.example' }],
    })
    second.resolve([stored, created])
    await Promise.all([one, two])
    expect(flow.read().name).toBe('Edited while saving')
    expect(completed).toHaveBeenCalledTimes(2)
  })

  it('rejects all waiters immediately on invalidate and consumes a late failed save without callbacks', async () => {
    const pending = Promise.withResolvers<Provider[]>()
    const flow = setup({ save: () => pending.promise })
    const one = flow.session.commit()
    const two = flow.session.commit()
    const rejected = Promise.all([
      expect(one).rejects.toMatchObject({ code: 'cancelled' }),
      expect(two).rejects.toMatchObject({ code: 'cancelled' }),
    ])
    flow.session.invalidate()
    flow.session.invalidate()
    await rejected
    await expect(flow.session.commit()).rejects.toMatchObject({ code: 'cancelled' })
    await expect(flow.session.probe('main')).rejects.toMatchObject({ code: 'cancelled' })
    pending.reject(new Error('late failure'))
    await Promise.resolve()
    expect(flow.onSaved).not.toHaveBeenCalled()
  })

  it('fails ambiguous or missing identities without adopting another provider and allows explicit retry', async () => {
    const save = vi
      .fn()
      .mockResolvedValueOnce([stored, { ...stored, id: 'other' }, { ...stored, id: 'third' }])
      .mockResolvedValueOnce([stored, { ...stored, id: 'created' }])
    const flow = setup({ save }, { ...toDraft(stored), id: null })
    await expect(flow.session.commit()).rejects.toBeInstanceOf(KitClientError)
    expect(flow.read().id).toBeNull()
    expect(flow.onSaved).not.toHaveBeenCalled()
    await flow.session.commit()
    expect(flow.read().id).toBe('created')
  })

  it('rejects missing endpoint identities without publishing an incomplete save response', async () => {
    const flow = setup({ save: async () => [{ ...stored, endpoints: [] }] })
    await expect(flow.session.commit()).rejects.toMatchObject({ code: 'internal' })
    expect(flow.read().endpoints[0]?.saved?.id).toBe(endpoint.id)
    expect(flow.onSaved).not.toHaveBeenCalled()
  })

  it('invalidates a running probe immediately and consumes its ignored late rejection', async () => {
    const late = Promise.withResolvers<ProbeResult>()
    const started = Promise.withResolvers<void>()
    const probe = vi.fn(() => {
      started.resolve()
      return late.promise
    })
    const flow = setup({ probe })
    const batch = flow.session.probe('main')
    const rejected = expect(batch).rejects.toMatchObject({ code: 'cancelled' })
    await started.promise
    flow.session.invalidate()
    await rejected
    late.reject(new Error('ignored late network failure'))
    await late.promise.catch(() => {})
    expect(probe).toHaveBeenCalledTimes(1)
  })

  it('keeps all queued commits failed and does not probe after a failed save barrier', async () => {
    const pending = Promise.withResolvers<Provider[]>()
    const probe = vi.fn()
    const save = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue([stored])
    const flow = setup({ save, probe })
    const one = flow.session.commit()
    const two = flow.session.probe('main')
    const failure = new Error('save failed')
    const rejected = Promise.all([expect(one).rejects.toBe(failure), expect(two).rejects.toBe(failure)])
    pending.reject(failure)
    await rejected
    expect(probe).not.toHaveBeenCalled()
    expect(save).toHaveBeenCalledTimes(1)
    await flow.session.commit()
    expect(save).toHaveBeenCalledTimes(2)
  })

  it('waits for save, snapshots keys and probes serially, retaining the first usable result', async () => {
    const pendingSave = Promise.withResolvers<Provider[]>()
    const first = Promise.withResolvers<ProbeResult>()
    const probe = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ ...result, probeUrl: 'second' })
    const flow = setup({ save: () => pendingSave.promise, probe })
    const batch = flow.session.probe('main')
    expect(probe).not.toHaveBeenCalled()
    pendingSave.resolve([stored])
    await pendingSave.promise
    await Promise.resolve()
    expect(probe).toHaveBeenCalledTimes(1)
    expect(probe.mock.calls[0]?.[0]).toMatchObject({ endpointId: endpoint.id, keyId: 'first' })
    first.resolve(result)
    expect((await batch).usable).toBe(result)
    expect(probe.mock.calls.map(([target]) => target.keyId)).toEqual(['first', 'second'])
  })

  it('aborts superseded probes and never sends their next key even if the port ignores abort', async () => {
    const late = Promise.withResolvers<ProbeResult>()
    const probe = vi.fn().mockReturnValueOnce(late.promise).mockResolvedValue(result)
    const flow = setup({ probe })
    const old = flow.session.probe('main')
    const rejected = expect(old).rejects.toMatchObject({ code: 'cancelled' })
    await Promise.resolve()
    await Promise.resolve()
    const current = flow.session.probe('main')
    await rejected
    await current
    late.resolve(result)
    await late.promise
    await Promise.resolve()
    expect(probe.mock.calls.map(([target]) => target.keyId)).toEqual(['first', 'first', 'second'])
    expect(probe.mock.calls[0]?.[1].signal.aborted).toBe(true)
  })

  it('rejects an aborted probe promptly while leaving its already submitted save intact', async () => {
    const pending = Promise.withResolvers<Provider[]>()
    const probe = vi.fn()
    const flow = setup({ save: () => pending.promise, probe })
    const controller = new AbortController()
    const batch = flow.session.probe('main', { signal: controller.signal })
    const rejected = expect(batch).rejects.toMatchObject({ code: 'cancelled' })
    controller.abort()
    await rejected
    pending.resolve([stored])
    await pending.promise
    await Promise.resolve()
    expect(probe).not.toHaveBeenCalled()
  })

  it('uses the default target for zero or one enabled key', async () => {
    for (const keys of [[], endpoint.keys.slice(0, 1)]) {
      const provider = { ...stored, endpoints: [{ ...endpoint, keys }] }
      const probe = vi.fn<(target: ProbeTarget) => Promise<ProbeResult>>(async () => result)
      const flow = setup({ probe, save: async () => [provider] }, toDraft(provider))
      expect((await flow.session.probe('main')).results).toEqual([{ keyId: null, result }])
      expect(probe.mock.calls[0]?.[0]).not.toHaveProperty('keyId')
    }
  })
})

describe('probe model merge', () => {
  it('deduplicates by id and keeps every current user field while new models use probe defaults', () => {
    const probed: ProbeResult['models'][number] = {
      id: 'model',
      name: 'Probe name',
      group: 'Probe group',
      groupName: null,
      modelType: 'chat',
      typeSource: 'inferred',
      contextWindow: 4096,
      maxOutput: 1024,
      reasoning: null,
      pricing: null,
      nativeWebSearch: false,
      canVision: false,
      supportsTools: false,
    }
    const metadata = { ...probed, groupName: probed.group }
    const existing = { ...metadata, groupName: 'User group', name: 'User name', canVision: true, supportsTools: true }
    const outside = { ...existing, id: 'outside' }
    const merged = mergeModels([existing, outside, outside], [probed, probed, { ...probed, id: 'new' }], {
      model: true,
      new: true,
    })
    expect(merged.map((model) => model.id)).toEqual(['outside', 'model', 'new'])
    expect(merged[1]).toBe(existing)
    expect(merged[2]).toMatchObject({ name: 'Probe name', groupName: 'Probe group' })
    expect(mergeModels([existing, outside], [probed], { model: false })).toEqual([outside])
    expect(mergeModels([existing], [probed], { model: true }, { model: 'image' })[0]).toMatchObject({
      modelType: 'image',
      typeSource: 'user',
      reasoning: null,
      pricing: null,
    })
  })
})
