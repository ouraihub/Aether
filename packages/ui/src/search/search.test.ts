import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts } from '../ports'
import type { CallOptions, SearchCap, SearchKey, SearchPreferences, SearchSave, SearchSettingsDto } from '../ports'
import Harness from './Harness.test.svelte'

const clipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')

beforeEach(() => {
  Object.defineProperty(HTMLDialogElement.prototype, 'showModal', {
    configurable: true,
    value: function (this: HTMLDialogElement) {
      this.open = true
    },
  })
  Object.defineProperty(HTMLDialogElement.prototype, 'close', {
    configurable: true,
    value: function (this: HTMLDialogElement) {
      this.open = false
      this.dispatchEvent(new Event('close'))
    },
  })
})
afterEach(() => {
  cleanup()
  if (clipboardDescriptor) Object.defineProperty(navigator, 'clipboard', clipboardDescriptor)
  else Reflect.deleteProperty(navigator, 'clipboard')
  delete (HTMLDialogElement.prototype as Partial<HTMLDialogElement>).showModal
  delete (HTMLDialogElement.prototype as Partial<HTMLDialogElement>).close
})

function mount() {
  const keys: SearchKey[] = [{ id: 'stored', masked: 'stored-mask' }]
  const preferences: SearchPreferences = {
    maxResults: 5,
    compression: 'none',
    cutoffLimit: 2000,
    blacklist: [],
    modelToolsPreferred: false,
  }
  const settings = (capability: SearchCap): SearchSettingsDto => ({
    capability,
    defaultProvider: 'jina',
    preferences,
    providers: [
      {
        id: 'jina',
        name: 'Jina',
        description: 'Reader',
        keyMode: 'optional',
        basicAuth: false,
        keys: [...keys],
        ready: true,
        defaultBaseUrl: 'https://jina.ai',
      },
    ],
  })
  const save = vi.fn<(input: SearchSave, options?: CallOptions) => Promise<SearchSettingsDto>>(async () =>
    settings('search'),
  )
  const saveKey = vi.fn(async (cap: SearchCap, _provider: string, _value: string, id?: string) => {
    if (!id) keys.push({ id: 'new', masked: 'new-mask' })
    return settings(cap)
  })
  const check = vi.fn(async () => ({ count: 2 }))
  const reveal = vi.fn(async () => 'private-key')
  const removeKey = vi.fn(async (cap: SearchCap, _provider: string, id: string) => {
    keys.splice(
      keys.findIndex((k) => k.id === id),
      1,
    )
    return settings(cap)
  })
  const savePreferences = vi.fn(async (input: SearchPreferences) => input)
  const ports = createFakePorts({
    search: {
      settings: async (cap) => settings(cap),
      save,
      saveKey,
      check,
      reveal,
      removeKey,
      savePreferences,
      revealNeedsPassword: true,
    },
  })
  const notify = { toast: vi.fn(), confirm: async () => true }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const view = render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } })
  return { view, save, saveKey, check, reveal, removeKey, savePreferences, notify, settings }
}

async function card() {
  return within(await screen.findByRole('region', { name: 'Web search' }))
}

test('visible key input saves before checking; empty input never deletes stored keys and failures keep draft', async () => {
  const h = mount()
  const c = await card()
  const input = c.getByLabelText('API key') as HTMLInputElement
  expect(input.type).toBe('password')
  await fireEvent.input(input, { target: { value: 'replacement' } })
  await fireEvent.click(c.getByRole('button', { name: 'Check' }))
  await waitFor(() => expect(h.check).toHaveBeenCalledOnce())
  expect(h.saveKey).toHaveBeenCalledWith('search', 'jina', 'replacement', undefined, expect.any(Object))
  expect(h.saveKey.mock.invocationCallOrder[0]).toBeLessThan(h.check.mock.invocationCallOrder[0] ?? 0)
  await fireEvent.click(c.getByRole('button', { name: 'Save' }))
  await waitFor(() => expect(h.save).toHaveBeenCalledTimes(2))
  expect(h.saveKey).toHaveBeenCalledTimes(1)
  h.save.mockRejectedValueOnce(new Error('save failed'))
  await fireEvent.input(input, { target: { value: 'keep-on-error' } })
  await fireEvent.click(c.getByRole('button', { name: 'Check' }))
  await waitFor(() => expect(c.getByRole('alert')).toBeTruthy())
  expect(h.check).toHaveBeenCalledTimes(1)
  expect(input.value).toBe('keep-on-error')
})

test('key manager updates scoped identity, requires password for reveal, and clears cleartext on close', async () => {
  const h = mount()
  const c = await card()
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  const dialog = await screen.findByRole('dialog', { name: /Manage keys.*Jina/ })
  const d = within(dialog)
  expect((d.getByRole('button', { name: 'Reveal' }) as HTMLButtonElement).disabled).toBe(true)
  await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'verified' } })
  await fireEvent.click(d.getByRole('button', { name: 'Reveal' }))
  await d.findByText('private-key')
  expect(h.reveal).toHaveBeenCalledWith('search', 'jina', 'stored', 'verified', expect.any(Object))
  await fireEvent.click(d.getByRole('button', { name: 'Edit key' }))
  await fireEvent.input(d.getByLabelText('Edit key'), { target: { value: 'updated' } })
  await fireEvent.click(d.getByRole('button', { name: 'Save' }))
  await waitFor(() => expect(h.saveKey).toHaveBeenCalledWith('search', 'jina', 'updated', 'stored', expect.any(Object)))
  await fireEvent.click(d.getByRole('button', { name: 'Cancel' }))
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  expect(screen.queryByText('private-key')).toBeNull()
  expect((within(screen.getByRole('dialog')).getByLabelText('Current password') as HTMLInputElement).value).toBe('')
})

test('advanced preferences preserve supported policies and submit blacklist with native search choice', async () => {
  const h = mount()
  await card()
  const details = screen.getByText('Advanced search settings').closest('details')
  expect(details?.open).toBe(false)
  await fireEvent.click(screen.getByText('Advanced search settings'))
  await fireEvent.change(screen.getByLabelText('Compression'), { target: { value: 'cutoff' } })
  await fireEvent.input(screen.getByLabelText('Cutoff characters'), { target: { value: '100' } })
  await fireEvent.input(screen.getByLabelText('Blacklist (one rule per line)'), {
    target: { value: 'example.com\n*://*.blocked.org/*' },
  })
  await fireEvent.click(screen.getByLabelText('Prefer native model web search'))
  const preferenceSection = details?.closest('section')
  if (!preferenceSection) throw new Error('Preferences section missing')
  await fireEvent.click(within(preferenceSection).getByRole('button', { name: 'Save' }))
  await waitFor(() =>
    expect(h.savePreferences).toHaveBeenCalledWith(
      {
        maxResults: 5,
        compression: 'cutoff',
        cutoffLimit: 100,
        blacklist: ['example.com', '*://*.blocked.org/*'],
        modelToolsPreferred: true,
      },
      expect.any(Object),
    ),
  )
})

test('config success is applied before key failure; retry keeps only the failed key draft', async () => {
  const h = mount()
  const c = await card()
  const url = c.getByLabelText('Base URL') as HTMLInputElement
  const key = c.getByLabelText('API key') as HTMLInputElement
  await fireEvent.input(url, { target: { value: 'https://saved.test' } })
  await fireEvent.input(key, { target: { value: 'retry-key' } })
  const saved = h.settings('search')
  saved.providers = saved.providers.map((provider, index) =>
    index === 0 ? { ...provider, baseUrl: 'https://saved.test' } : provider,
  )
  h.save.mockResolvedValueOnce(saved)
  h.saveKey.mockRejectedValueOnce(new Error('key failed'))
  await fireEvent.click(c.getByRole('button', { name: 'Check' }))
  await waitFor(() => expect(c.getByRole('alert')).toBeTruthy())
  expect(url.value).toBe('https://saved.test')
  expect(key.value).toBe('retry-key')
  expect(h.check).not.toHaveBeenCalled()
  await fireEvent.click(c.getByRole('button', { name: 'Save' }))
  await waitFor(() => expect(h.save).toHaveBeenCalledTimes(2))
  expect(h.save.mock.calls[1]?.[0]).toEqual({ capability: 'search', provider: 'jina' })
  await waitFor(() => expect(key.value).toBe(''))
})

test('key success clears plaintext immediately even while check is pending and remains cleared on check failure', async () => {
  const h = mount()
  const c = await card()
  const key = c.getByLabelText('API key') as HTMLInputElement
  const checking = Promise.withResolvers<{ count: number }>()
  h.check.mockReturnValueOnce(checking.promise)
  await fireEvent.input(key, { target: { value: 'saved-key' } })
  await fireEvent.click(c.getByRole('button', { name: 'Check' }))
  await waitFor(() => expect(h.check).toHaveBeenCalledOnce())
  expect(key.value).toBe('')
  checking.reject(new Error('check failed'))
  await waitFor(() => expect(c.getByRole('alert')).toBeTruthy())
  expect(key.value).toBe('')
})

test('same-text edits made during save survive config and key success by revision', async () => {
  const h = mount()
  const c = await card()
  const url = c.getByLabelText('Base URL') as HTMLInputElement
  const key = c.getByLabelText('API key') as HTMLInputElement
  const saving = Promise.withResolvers<SearchSettingsDto>()
  h.save.mockReturnValueOnce(saving.promise)
  await fireEvent.input(url, { target: { value: 'https://draft.test' } })
  await fireEvent.input(key, { target: { value: 'same-secret' } })
  await fireEvent.click(c.getByRole('button', { name: 'Save' }))
  // Programmatic input represents an edit arriving after the captured save revision.
  await fireEvent.input(url, { target: { value: 'https://new.test' } })
  await fireEvent.input(url, { target: { value: 'https://draft.test' } })
  await fireEvent.input(key, { target: { value: 'new-secret' } })
  await fireEvent.input(key, { target: { value: 'same-secret' } })
  saving.resolve(h.settings('search'))
  await waitFor(() => expect(h.notify.toast).toHaveBeenCalled())
  expect(url.value).toBe('https://draft.test')
  expect(key.value).toBe('same-secret')
})

test('reveal success and failure clear only the password revision consumed by that request', async () => {
  const h = mount()
  const c = await card()
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  const d = within(screen.getByRole('dialog'))
  const password = d.getByLabelText('Current password') as HTMLInputElement
  await fireEvent.input(password, { target: { value: 'one-use' } })
  await fireEvent.click(d.getByRole('button', { name: 'Reveal' }))
  await d.findByText('private-key')
  expect(password.value).toBe('')
  const revealing = Promise.withResolvers<string>()
  h.reveal.mockReturnValueOnce(revealing.promise)
  await fireEvent.input(password, { target: { value: 'same-password' } })
  await fireEvent.click(d.getByRole('button', { name: 'Reveal' }))
  await fireEvent.input(password, { target: { value: 'different-password' } })
  await fireEvent.input(password, { target: { value: 'same-password' } })
  revealing.reject(new Error('failed'))
  await waitFor(() => expect(d.getByRole('alert')).toBeTruthy())
  expect(password.value).toBe('same-password')
  h.reveal.mockRejectedValueOnce(new Error('failed again'))
  await fireEvent.click(d.getByRole('button', { name: 'Reveal' }))
  await waitFor(() => expect(password.value).toBe(''))
})

for (const action of ['Reveal', 'Copy'] as const) {
  test(`late ${action.toLowerCase()} result cannot expose plaintext or clear a new target password`, async () => {
    const h = mount()
    const c = await card()
    const revealing = Promise.withResolvers<string>()
    h.reveal.mockReturnValueOnce(revealing.promise)
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
    let d = within(screen.getByRole('dialog'))
    await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'old-password' } })
    await fireEvent.click(d.getByRole('button', { name: action }))
    await fireEvent.click(d.getByRole('button', { name: 'Cancel' }))
    // The other capability remains available while the old target is pending.
    const other = within(screen.getByRole('region', { name: 'Read webpage' }))
    await fireEvent.click(other.getByRole('button', { name: 'Manage keys' }))
    d = within(screen.getByRole('dialog'))
    await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'new-password' } })
    revealing.resolve('late-secret')
    await waitFor(() =>
      expect((c.getByRole('button', { name: 'Manage keys' }) as HTMLButtonElement).disabled).toBe(false),
    )
    expect(screen.queryByText('late-secret')).toBeNull()
    expect((d.getByLabelText('Current password') as HTMLInputElement).value).toBe('new-password')
    expect(writeText).not.toHaveBeenCalled()
    expect(h.notify.toast).not.toHaveBeenCalled()
  })
}

test('clipboard completion after close cannot toast into the new dialog', async () => {
  const h = mount()
  const c = await card()
  const copying = Promise.withResolvers<void>()
  const writeText = vi.fn(() => copying.promise)
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  const d = within(screen.getByRole('dialog'))
  await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'one-use' } })
  await fireEvent.click(d.getByRole('button', { name: 'Copy' }))
  await waitFor(() => expect(writeText).toHaveBeenCalledWith('private-key'))
  await fireEvent.click(d.getByRole('button', { name: 'Cancel' }))
  const other = within(screen.getByRole('region', { name: 'Read webpage' }))
  await fireEvent.click(other.getByRole('button', { name: 'Manage keys' }))
  copying.resolve()
  await waitFor(() =>
    expect((c.getByRole('button', { name: 'Manage keys' }) as HTMLButtonElement).disabled).toBe(false),
  )
  expect(h.notify.toast).not.toHaveBeenCalled()
})

for (const fail of [false, true]) {
  test(`late key save ${fail ? 'failure' : 'success'} leaves reopened dialog edits and settings untouched`, async () => {
    const h = mount()
    const c = await card()
    const saving = Promise.withResolvers<SearchSettingsDto>()
    h.saveKey.mockReturnValueOnce(saving.promise)
    await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
    let d = within(screen.getByRole('dialog'))
    await fireEvent.click(d.getByRole('button', { name: 'Edit key' }))
    await fireEvent.input(d.getByLabelText('Edit key'), { target: { value: 'old-edit' } })
    await fireEvent.click(d.getByRole('button', { name: 'Save' }))
    await fireEvent.click(d.getByRole('button', { name: 'Cancel' }))
    const other = within(screen.getByRole('region', { name: 'Read webpage' }))
    await fireEvent.click(other.getByRole('button', { name: 'Manage keys' }))
    d = within(screen.getByRole('dialog'))
    await fireEvent.input(d.getByLabelText('New key'), { target: { value: 'new-edit' } })
    if (fail) saving.reject(new Error('late failure'))
    else {
      const stale = h.settings('search')
      stale.providers = stale.providers.map((provider, index) =>
        index === 0 ? { ...provider, keys: [{ id: 'late', masked: 'late-mask' }] } : provider,
      )
      saving.resolve(stale)
    }
    await waitFor(() =>
      expect((c.getByRole('button', { name: 'Manage keys' }) as HTMLButtonElement).disabled).toBe(false),
    )
    expect((d.getByLabelText('New key') as HTMLInputElement).value).toBe('new-edit')
    expect(d.queryByRole('alert')).toBeNull()
    expect(d.queryByText('late-mask')).toBeNull()
    expect(d.getByText('stored-mask')).toBeTruthy()
  })
}

test('closing the key dialog clears its secrets and the corresponding inline key draft', async () => {
  mount()
  const c = await card()
  const input = c.getByLabelText('API key') as HTMLInputElement
  await fireEvent.input(input, { target: { value: 'inline-secret' } })
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  let d = within(screen.getByRole('dialog'))
  await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'password-secret' } })
  await fireEvent.input(d.getByLabelText('New key'), { target: { value: 'edit-secret' } })
  await fireEvent.click(d.getByRole('button', { name: 'Cancel' }))
  expect(input.value).toBe('')
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  d = within(screen.getByRole('dialog'))
  expect((d.getByLabelText('Current password') as HTMLInputElement).value).toBe('')
  expect((d.getByLabelText('New key') as HTMLInputElement).value).toBe('')
})

test('same-target reopen after a closed reveal failure starts a clean generation', async () => {
  const h = mount()
  const c = await card()
  const revealing = Promise.withResolvers<string>()
  h.reveal.mockReturnValueOnce(revealing.promise)
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  let d = within(screen.getByRole('dialog'))
  await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'old-password' } })
  await fireEvent.click(d.getByRole('button', { name: 'Reveal' }))
  await fireEvent.click(d.getByRole('button', { name: 'Cancel' }))
  expect((c.getByRole('button', { name: 'Manage keys' }) as HTMLButtonElement).disabled).toBe(true)
  revealing.reject(new Error('closed request failed'))
  await waitFor(() =>
    expect((c.getByRole('button', { name: 'Manage keys' }) as HTMLButtonElement).disabled).toBe(false),
  )
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  d = within(screen.getByRole('dialog'))
  expect(d.queryByRole('alert')).toBeNull()
  expect((d.getByLabelText('Current password') as HTMLInputElement).value).toBe('')
  expect((d.getByLabelText('New key') as HTMLInputElement).value).toBe('')
})

test('key save cleanup preserves a newly edited identical value and replacement clears revealed secrets', async () => {
  const h = mount()
  const c = await card()
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  const d = within(screen.getByRole('dialog'))
  await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'one-use' } })
  await fireEvent.click(d.getByRole('button', { name: 'Reveal' }))
  await d.findByText('private-key')
  await fireEvent.click(d.getByRole('button', { name: 'Edit key' }))
  expect(d.queryByText('private-key')).toBeNull()
  const edit = d.getByLabelText('Edit key') as HTMLInputElement
  await fireEvent.input(edit, { target: { value: 'same-edit' } })
  const saving = Promise.withResolvers<SearchSettingsDto>()
  h.saveKey.mockReturnValueOnce(saving.promise)
  await fireEvent.click(d.getByRole('button', { name: 'Save' }))
  await fireEvent.input(edit, { target: { value: 'other-edit' } })
  await fireEvent.input(edit, { target: { value: 'same-edit' } })
  saving.resolve(h.settings('search'))
  await waitFor(() => expect((d.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(false))
  expect(edit.value).toBe('same-edit')
})

test('unmount during save prevents key and check steps and notifications', async () => {
  const h = mount()
  const c = await card()
  const saving = Promise.withResolvers<SearchSettingsDto>()
  h.save.mockReturnValueOnce(saving.promise)
  await fireEvent.input(c.getByLabelText('API key'), { target: { value: 'unmounted-secret' } })
  await fireEvent.click(c.getByRole('button', { name: 'Check' }))
  h.view.unmount()
  saving.resolve(h.settings('search'))
  await saving.promise
  // Let the cancelled flow consume the late response before observing callbacks.
  await Promise.resolve()
  expect(h.saveKey).not.toHaveBeenCalled()
  expect(h.check).not.toHaveBeenCalled()
  expect(h.notify.toast).not.toHaveBeenCalled()
})

test('unmount during reveal prevents clipboard writes and secret notifications', async () => {
  const h = mount()
  const c = await card()
  const revealing = Promise.withResolvers<string>()
  const writeText = vi.fn(async () => {})
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  h.reveal.mockReturnValueOnce(revealing.promise)
  await fireEvent.click(c.getByRole('button', { name: 'Manage keys' }))
  const d = within(screen.getByRole('dialog'))
  await fireEvent.input(d.getByLabelText('Current password'), { target: { value: 'unmounted-password' } })
  await fireEvent.click(d.getByRole('button', { name: 'Copy' }))
  h.view.unmount()
  revealing.resolve('unmounted-secret')
  await revealing.promise
  await Promise.resolve()
  expect(writeText).not.toHaveBeenCalled()
  expect(h.notify.toast).not.toHaveBeenCalled()
})
