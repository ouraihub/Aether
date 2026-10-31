import { expect, test, vi } from 'vitest'
import { createFakePorts, KitClientError } from '../ports'
import type { SearchSettingsDto } from '../ports'
import { saveSearchSettings } from './settings-flow'

function setup() {
  const settings: SearchSettingsDto = {
    capability: 'search',
    defaultProvider: 'jina',
    providers: [],
    preferences: {
      maxResults: 5,
      compression: 'none',
      cutoffLimit: 2000,
      blacklist: [],
      modelToolsPreferred: false,
    },
  }
  const config = {
    capability: 'search' as const,
    provider: 'jina',
    baseUrl: 'https://example.test',
  }
  const save = vi.fn(async () => settings)
  const saveKey = vi.fn(async () => ({
    ...settings,
    defaultProvider: 'saved-key',
  }))
  const check = vi.fn(async () => ({ count: 3 }))
  const port = createFakePorts({ search: { save, saveKey, check } }).search
  if (!port) throw new Error('Expected search fixture port')
  let current = true
  const session = {
    isCurrent: () => current,
    onSettings: vi.fn(),
    onConfigSaved: vi.fn(),
    onKeySaved: vi.fn(),
  }
  return {
    settings,
    config,
    save,
    saveKey,
    check,
    port,
    session,
    invalidate: () => {
      current = false
    },
  }
}

for (const failedStep of ['save', 'saveKey', 'check'] as const) {
  test(`${failedStep} failure preserves only already persisted callbacks and the original error`, async () => {
    const h = setup()
    const failure = new KitClientError({
      code: 'internal',
      message: failedStep,
      details: {},
    })
    h[failedStep].mockRejectedValueOnce(failure)
    await expect(
      saveSearchSettings(
        h.port,
        {
          config: h.config,
          key: { value: 'example-secret', keyId: 'existing' },
          check: true,
        },
        h.session,
      ),
    ).rejects.toBe(failure)
    expect(h.session.onConfigSaved).toHaveBeenCalledTimes(
      failedStep === 'save' ? 0 : 1,
    )
    expect(h.session.onKeySaved).toHaveBeenCalledTimes(
      failedStep === 'check' ? 1 : 0,
    )
    expect(h.session.onSettings).toHaveBeenCalledTimes(
      failedStep === 'save' ? 0 : failedStep === 'saveKey' ? 1 : 2,
    )
    expect(h.saveKey).toHaveBeenCalledTimes(failedStep === 'save' ? 0 : 1)
    expect(h.check).toHaveBeenCalledTimes(failedStep === 'check' ? 1 : 0)
  })
}

test('config and key callbacks apply immediately, before a pending check settles', async () => {
  const h = setup()
  const pendingCheck = Promise.withResolvers<{ count: number }>()
  h.check.mockReturnValueOnce(pendingCheck.promise)
  const called = Promise.withResolvers<void>()
  h.session.onKeySaved.mockImplementation(() => called.resolve())
  const saving = saveSearchSettings(
    h.port,
    { config: h.config, key: { value: 'example-secret' }, check: true },
    h.session,
  )
  await called.promise
  expect(h.session.onConfigSaved).toHaveBeenCalledWith(h.config)
  expect(h.session.onSettings).toHaveBeenLastCalledWith({
    ...h.settings,
    defaultProvider: 'saved-key',
  })
  pendingCheck.resolve({ count: 4 })
  await expect(saving).resolves.toEqual({
    settings: { ...h.settings, defaultProvider: 'saved-key' },
    checked: { count: 4 },
  })
})

test('empty key and optional check are skipped', async () => {
  const h = setup()
  await expect(
    saveSearchSettings(
      h.port,
      { config: h.config, key: { value: '  ' }, check: false },
      h.session,
    ),
  ).resolves.toEqual({ settings: h.settings })
  expect(h.saveKey).not.toHaveBeenCalled()
  expect(h.check).not.toHaveBeenCalled()
})

for (const lateFailure of [false, true]) {
  test(`invalidation rejects cancelled and ignores late config ${lateFailure ? 'failure' : 'success'}`, async () => {
    const h = setup()
    const pending = Promise.withResolvers<SearchSettingsDto>()
    h.save.mockReturnValueOnce(pending.promise)
    const saving = saveSearchSettings(
      h.port,
      { config: h.config, key: { value: 'example-secret' }, check: true },
      h.session,
    )
    const rejected = expect(saving).rejects.toMatchObject({ code: 'cancelled' })
    h.invalidate()
    if (lateFailure) pending.reject(new Error('late'))
    else pending.resolve(h.settings)
    await rejected
    expect(h.session.onSettings).not.toHaveBeenCalled()
    expect(h.session.onConfigSaved).not.toHaveBeenCalled()
    expect(h.saveKey).not.toHaveBeenCalled()
    expect(h.check).not.toHaveBeenCalled()
  })
}

test('abort rejects without waiting for an uncooperative port and consumes its late rejection', async () => {
  const h = setup()
  const pending = Promise.withResolvers<SearchSettingsDto>()
  h.save.mockReturnValueOnce(pending.promise)
  const controller = new AbortController()
  const options = { signal: controller.signal }
  const saving = saveSearchSettings(
    h.port,
    { config: h.config, key: { value: 'example-secret' }, check: true },
    h.session,
    options,
  )
  const rejected = expect(saving).rejects.toMatchObject({ code: 'cancelled' })
  controller.abort()
  await rejected
  pending.reject(new Error('late'))
  expect(h.save).toHaveBeenCalledWith(h.config, options)
  expect(h.session.onSettings).not.toHaveBeenCalled()
  expect(h.saveKey).not.toHaveBeenCalled()
})

test('cancellation from a partial-success callback prevents the next step', async () => {
  const h = setup()
  h.session.onKeySaved.mockImplementation(h.invalidate)
  await expect(
    saveSearchSettings(
      h.port,
      { config: h.config, key: { value: 'example-secret' }, check: true },
      h.session,
    ),
  ).rejects.toMatchObject({ code: 'cancelled' })
  expect(h.session.onConfigSaved).toHaveBeenCalledOnce()
  expect(h.session.onKeySaved).toHaveBeenCalledOnce()
  expect(h.check).not.toHaveBeenCalled()
})

test('already cancelled calls never reach the port', async () => {
  const h = setup()
  h.invalidate()
  await expect(
    saveSearchSettings(h.port, { config: h.config, check: false }, h.session),
  ).rejects.toMatchObject({
    code: 'cancelled',
  })
  expect(h.save).not.toHaveBeenCalled()
})

for (const stage of ['saveKey', 'check'] as const) {
  test(`abort during ${stage} keeps prior successes and ignores the late response`, async () => {
    const h = setup()
    const entered = Promise.withResolvers<void>()
    const pending = Promise.withResolvers<
      SearchSettingsDto & { count: number }
    >()
    h[stage].mockImplementationOnce(() => {
      entered.resolve()
      return pending.promise
    })
    const controller = new AbortController()
    const saving = saveSearchSettings(
      h.port,
      { config: h.config, key: { value: 'example-secret' }, check: true },
      h.session,
      { signal: controller.signal },
    )
    const rejected = expect(saving).rejects.toMatchObject({ code: 'cancelled' })
    await entered.promise
    controller.abort()
    await rejected
    pending.resolve({ ...h.settings, count: 7 })
    expect(h.session.onConfigSaved).toHaveBeenCalledOnce()
    expect(h.session.onKeySaved).toHaveBeenCalledTimes(
      stage === 'check' ? 1 : 0,
    )
    expect(h.session.onSettings).toHaveBeenCalledTimes(
      stage === 'check' ? 2 : 1,
    )
    expect(h.check).toHaveBeenCalledTimes(stage === 'check' ? 1 : 0)
  })
}

test('each request keeps the originally captured config and key identity', async () => {
  const h = setup()
  const pending = Promise.withResolvers<SearchSettingsDto>()
  h.save.mockReturnValueOnce(pending.promise)
  const input = {
    config: { ...h.config },
    key: { value: 'original-secret', keyId: 'original-key' },
    check: true,
  }
  const saving = saveSearchSettings(h.port, input, h.session)
  input.config.provider = 'other-provider'
  input.key.value = 'new-secret'
  input.key.keyId = 'new-key'
  input.check = false
  pending.resolve(h.settings)
  await saving
  expect(h.saveKey).toHaveBeenCalledWith(
    'search',
    'jina',
    'original-secret',
    'original-key',
    undefined,
  )
  expect(h.check).toHaveBeenCalledWith('search', 'jina', undefined)
})
