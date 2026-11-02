import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError, type Endpoint, type Model, type Provider } from '../ports'
import { toDraft, toSave } from './draft'
import { endpoint, model, setup, stored, twoKeys } from './test-support/providers-fixture'

async function openPicker() {
  await fireEvent.click(await screen.findByRole('button', { name: 'Sync models' }))
  return within(await screen.findByRole('dialog', { name: 'Pick models' }))
}

describe('ProvidersSettings', () => {
  it('does not apply a previous provider save or error after selecting a different provider', async () => {
    const pending = Promise.withResolvers<Provider[]>()
    const other: Provider = {
      ...stored,
      id: 'p2',
      name: 'Other',
      endpoints: [{ ...endpoint, id: 'e2' }],
    }
    const { notify } = setup({ list: async () => [stored, other], save: () => pending.promise })
    const url = await screen.findByRole('textbox', { name: 'API URL' })
    await fireEvent.input(url, { target: { value: 'https://old-edit.example' } })
    await fireEvent.blur(url)
    await fireEvent.click(screen.getByText('Other'))
    pending.reject(new Error('old provider save failure'))
    await pending.promise.catch(() => {})
    await Promise.resolve()
    expect((screen.getByRole('textbox', { name: 'API URL' }) as HTMLInputElement).value).toBe(
      endpoint.baseUrl,
    )
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull()
    expect(notify.toast).not.toHaveBeenCalled()
  })

  it('never probes or opens a picker when its prerequisite save fails', async () => {
    const probe = vi.fn()
    const { notify } = setup({
      save: async () => {
        throw new Error('save barrier failed')
      },
      probe,
    })
    await fireEvent.click(await screen.findByRole('button', { name: 'Sync models' }))
    await waitFor(() => expect(screen.getByText('Error: save barrier failed')).toBeTruthy())
    expect(probe).not.toHaveBeenCalled()
    expect(screen.queryByRole('dialog', { name: 'Pick models' })).toBeNull()
    expect(notify.toast).not.toHaveBeenCalled()
  })
  it('saves each +/- at once with the endpoint id and ', async () => {
    const { saves } = setup()
    const picker = await openPicker()
    await fireEvent.click(picker.getByRole('button', { name: 'Add gpt-4o' }))
    await waitFor(() => expect(saves).toHaveLength(2))
    expect(screen.queryByRole('button', { name: /Use \d+ models/ })).toBeNull()
    const saved = saves.at(-1)?.endpoints[0]
    expect(saved?.id).toBe('e1')
    expect(saved?.models.map((m) => m.id)).toEqual(['gpt-4o'])
  })

  it('bulk changes only search results and preserves picks outside the filter', async () => {
    const { saves } = setup()
    const picker = await openPicker()
    await fireEvent.click(picker.getByRole('button', { name: 'Add gpt-4o' }))
    await fireEvent.input(picker.getByRole('textbox', { name: 'Search model ID' }), {
      target: { value: 'embedding' },
    })
    await fireEvent.click(picker.getByRole('button', { name: 'Add all' }))
    await fireEvent.click(picker.getByRole('button', { name: 'Remove all' }))
    await fireEvent.input(picker.getByRole('textbox', { name: 'Search model ID' }), {
      target: { value: '' },
    })
    expect(picker.getByRole('button', { name: 'Remove gpt-4o' })).toBeTruthy()
    expect(picker.getByRole('button', { name: 'Add text-embedding-3' })).toBeTruthy()
    await waitFor(() =>
      expect(saves.at(-1)?.endpoints[0]?.models.map((m) => m.id)).toEqual(['gpt-4o']),
    )
    await fireEvent.click(picker.getByRole('button', { name: 'Remove gpt-4o' }))
    await waitFor(() => expect(saves.at(-1)?.endpoints[0]?.models).toEqual([]))
  })

  it('toggles enabled immediately without a whole-provider save', async () => {
    const setEnabled = vi.fn((_id: string, enabled: boolean) =>
      Promise.resolve([{ ...stored, enabled }]),
    )
    const { saves } = setup({ setEnabled })
    await fireEvent.click(await screen.findByRole('switch', { name: 'Enabled' }))
    await waitFor(() => expect(setEnabled).toHaveBeenCalledWith('p1', false))
    expect(saves).toHaveLength(0)
  })

  it('copies the key by revealing it first when it is still masked', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const revealKey = vi.fn(() => Promise.resolve('sk-plain-key'))
    setup({ revealKey })
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy key' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('sk-plain-key'))
    expect(revealKey).toHaveBeenCalledWith('k1', '')
  })

  it('asks for the web password in a field browsers will not autofill', async () => {
    setup({ revealNeedsPassword: true, revealKey: vi.fn(() => Promise.resolve('sk-plain-key')) })
    await fireEvent.click(await screen.findByRole('button', { name: 'Show key' }))
    const password = document.querySelector('input[aria-label="Current password"]')
    expect(password?.getAttribute('autocomplete')).toBe('new-password')
    expect((password as HTMLInputElement | null)?.value).toBe('')
    expect(
      screen.getByRole('textbox', { name: 'Search model platforms…' }).getAttribute('autocomplete'),
    ).toBe('off')
  })

  it('edits the stored key without browser autofill', async () => {
    const revealKey = vi.fn(() => Promise.resolve('sk-plain-key'))
    setup({ revealKey })
    await fireEvent.click(await screen.findByRole('button', { name: 'Manage API keys' }))
    const dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Edit key' }))
    await waitFor(() =>
      expect(
        (document.querySelector('input[name="provider-api-key"]') as HTMLInputElement | null)
          ?.value,
      ).toBe('sk-plain-key'),
    )
    const keyInput = document.querySelector('input[name="provider-api-key"]')
    expect(revealKey).toHaveBeenCalledWith('k1', '')
    expect(keyInput?.getAttribute('type')).toBe('text')
    expect(keyInput?.getAttribute('autocomplete')).toBe('off')
  })

  it('copies the trimmed API address from the input copy button', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    setup()
    const url = (await screen.findByRole('textbox', { name: 'API URL' })) as HTMLInputElement
    await fireEvent.input(url, { target: { value: '  https://copy.example.com/v1  ' } })
    await fireEvent.click(
      within(url.parentElement as HTMLElement).getByRole('button', { name: 'Copy' }),
    )
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('https://copy.example.com/v1'))
  })

  it('row menu edits model notes and deletes the provider', async () => {
    const remove = vi.fn(() => Promise.resolve([]))
    const { saves } = setup({ remove })
    await fireEvent.click(await screen.findByRole('button', { name: 'More actions for Local' }))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Model notes' }))
    await fireEvent.input(await screen.findByRole('textbox', { name: 'Model notes' }), {
      target: { value: '## 价格\n- 便宜' },
    })
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves.at(-1)?.notes).toBe('## 价格\n- 便宜'))
    await fireEvent.contextMenu(
      screen.getByRole('button', { name: 'More actions for Local' }).closest('li') as HTMLElement,
    )
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }))
    await waitFor(() => expect(remove).toHaveBeenCalledWith('p1'))
  })

  it('shows the primary key with +N and offers delete only for a single key', async () => {
    setup({}, { ...stored, endpoints: [twoKeys] })
    expect(await screen.findByText('+1')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Delete key' })).toBeNull()
  })

  it('deletes the only key from the row after confirming', async () => {
    const removeKey = vi.fn(() => Promise.resolve([stored]))
    setup({ removeKey })
    expect(screen.queryByText('+1')).toBeNull()
    await fireEvent.click(await screen.findByRole('button', { name: 'Delete key' }))
    await waitFor(() => expect(removeKey).toHaveBeenCalledWith('e1', 'k1'))
  })

  it('adds no key without text and toggles a key from the dialog', async () => {
    const saveKey = vi.fn(() => Promise.resolve([{ ...stored, endpoints: [twoKeys] }]))
    const { notify } = setup({ saveKey }, { ...stored, endpoints: [twoKeys] })
    await fireEvent.click(await screen.findByRole('button', { name: 'Manage API keys' }))
    const dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    expect(dialog.getByText('2 / 2 enabled')).toBeTruthy()
    await fireEvent.click(dialog.getByRole('button', { name: 'Add API key' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    expect(notify.toast).toHaveBeenCalledWith('error', 'API key is required')
    expect(saveKey).not.toHaveBeenCalled()
    await fireEvent.click(dialog.getByRole('button', { name: 'Cancel' }))
    await fireEvent.click(dialog.getByRole('switch', { name: 'Enable backup' }))
    await waitFor(() =>
      expect(saveKey).toHaveBeenCalledWith({ endpointId: 'e1', id: 'k2', enabled: false }),
    )
  })

  it('offers the key strategy only when the host stores it, and saves the chosen strategy', async () => {
    setup()
    await screen.findByRole('button', { name: 'Manage API keys' })
    expect(screen.queryByLabelText('Key use')).toBeNull()
    cleanup()
    setup({ keyStrategies: true })
    expect((await screen.findByLabelText('Key use')).textContent).toContain('Rotate')
    const draft = toDraft({ ...stored, endpoints: [{ ...endpoint, keyStrategy: 'fill_first' }] })
    expect(toSave(draft).endpoints[0]).toMatchObject({ id: 'e1', keyStrategy: 'fill_first' })
  })

  it('shows a set-aside key with its recovery time and restores it', async () => {
    const later = Date.now() + 3_600_000
    const backup = twoKeys.keys[1]
    if (!backup) throw new Error('missing key')
    const paused: Endpoint = {
      ...twoKeys,
      keyStrategy: 'fill_first',
      keys: [
        twoKeys.keys[0] ?? backup,
        { ...backup, exhaustedUntil: later, exhaustedReason: 'HTTP 429' },
      ],
    }
    const saveKey = vi.fn(() => Promise.resolve([{ ...stored, endpoints: [twoKeys] }]))
    setup({ saveKey, keyStrategies: true }, { ...stored, endpoints: [paused] })
    await fireEvent.click(await screen.findByRole('button', { name: 'Manage API keys' }))
    const dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    expect(dialog.getByText(/^Paused until .* · HTTP 429$/)).toBeTruthy()
    await fireEvent.click(dialog.getByRole('button', { name: 'Restore' }))
    await waitFor(() =>
      expect(saveKey).toHaveBeenCalledWith({ endpointId: 'e1', id: 'k2', restore: true }),
    )
  })

  it('commits a changed address on blur and rejects an invalid one without a request', async () => {
    const { saves } = setup()
    const url = await screen.findByRole('textbox', { name: 'API URL' })
    await fireEvent.input(url, { target: { value: 'ftp://nope' } })
    await fireEvent.blur(url)
    expect(await screen.findByRole('alert')).toBeTruthy()
    expect(saves).toHaveLength(0)
    await fireEvent.input(url, { target: { value: 'https://api.example.com/v1' } })
    await fireEvent.blur(url)
    await waitFor(() => expect(saves).toHaveLength(1))
    expect(saves[0]?.endpoints[0]).toMatchObject({
      id: 'e1',
      baseUrl: 'https://api.example.com/v1',
    })
  })

  it('offers the suggested protocol after a check and saves the switch', async () => {
    const probe = vi.fn(() =>
      Promise.resolve({
        probeUrl: endpoint.probeUrl,
        error: null,
        models: [],
        suggestedProtocol: 'anthropic-messages' as const,
      }),
    )
    const { saves } = setup({ probe })
    await fireEvent.click(await screen.findByRole('button', { name: 'Check' }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Use Anthropic protocol' }))
    await waitFor(() => expect(saves).toHaveLength(2))
    expect(saves.at(-1)?.endpoints[0]).toMatchObject({ id: 'e1', protocol: 'anthropic-messages' })
    expect(screen.queryByRole('button', { name: 'Use Anthropic protocol' })).toBeNull()
  })

  it('keeps local edits and offers a retry when a save fails', async () => {
    const save = vi.fn(() => Promise.reject(new Error('down')))
    const { notify } = setup({ save })
    const url = await screen.findByRole('textbox', { name: 'API URL' })
    await fireEvent.input(url, { target: { value: 'https://other.example.com' } })
    await fireEvent.blur(url)
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Error: down'))
    expect((url as HTMLInputElement).value).toBe('https://other.example.com')
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(save).toHaveBeenCalledTimes(2)
  })

  async function openConfig() {
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Configure API URL and custom headers' }),
    )
    return within(await screen.findByRole('dialog', { name: 'Request configuration' }))
  }

  it('discards request config edits on cancel', async () => {
    const { saves } = setup()
    let dialog = await openConfig()
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Image generation base URL' }), {
      target: { value: 'https://img.example.com' },
    })
    await fireEvent.click(dialog.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(saves).toHaveLength(0)
    dialog = await openConfig()
    expect(
      (dialog.getByRole('textbox', { name: 'Image generation base URL' }) as HTMLInputElement)
        .value,
    ).toBe('')
  })

  it('saves image base URL and headers with the provider', async () => {
    const { saves } = setup()
    const dialog = await openConfig()
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Image generation base URL' }), {
      target: { value: ' https://img.example.com ' },
    })
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Image edit base URL' }), {
      target: { value: ' https://edit.example.com ' },
    })
    await fireEvent.click(dialog.getByRole('button', { name: 'Add header' }))
    const [, name = document.body] = dialog.getAllByRole('textbox', { name: 'Header name' })
    const [, value = document.body] = dialog.getAllByRole('textbox', { name: 'Header value' })
    await fireEvent.input(name, { target: { value: 'X-Trace' } })
    await fireEvent.input(value, { target: { value: '1' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves).toHaveLength(1))
    expect(saves[0]?.endpoints[0]).toMatchObject({
      id: 'e1',
      imageBaseUrl: 'https://img.example.com',
      imageEditBaseUrl: 'https://edit.example.com',
      headers: { 'X-Team': 'a', 'X-Trace': '1' },
    })
  })

  it('blocks an invalid image edit base URL', async () => {
    const { saves } = setup()
    const dialog = await openConfig()
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Image edit base URL' }), {
      target: { value: 'ftp://edit' },
    })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    expect(dialog.getByRole('alert')).toBeTruthy()
    expect(saves).toHaveLength(0)
  })

  const typed = { ...model, typeSource: 'user', reasoning: null } as const

  it('maps purpose onto type and vision, edits name and keeps untouched reasoning out of the save', async () => {
    const models: Model[] = [
      { ...typed, id: 'gpt-image-1', modelType: 'image' },
      {
        ...typed,
        id: 'gpt-4o',
        modelType: 'chat',
        reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
      },
    ]
    const { saves } = setup({}, { ...stored, endpoints: [{ ...endpoint, models }] })
    await fireEvent.click(await screen.findByRole('button', { name: 'Settings for gpt-4o' }))
    let dialog = within(await screen.findByRole('dialog', { name: 'Edit model' }))
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Model name' }), {
      target: { value: '  GPT 4o  ' },
    })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves).toHaveLength(1))
    const saved = saves[0]?.endpoints[0]?.models.find((m) => m.id === 'gpt-4o')
    expect(saved?.name).toBe('GPT 4o')
    expect(saved && 'reasoning' in saved).toBe(false)
    expect(await screen.findByText('GPT 4o')).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: 'Settings for gpt-image-1' }))
    dialog = within(await screen.findByRole('dialog', { name: 'Edit model' }))
    expect(
      (dialog.getByRole('radio', { name: 'Image generation' }) as HTMLInputElement).checked,
    ).toBe(true)
    expect(dialog.queryByRole('button', { name: 'Vision' })).toBeNull()
    await fireEvent.click(dialog.getByRole('radio', { name: 'Image editing' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves).toHaveLength(2))
    expect(saves[1]?.endpoints[0]?.models.find((m) => m.id === 'gpt-image-1')).toMatchObject({
      modelType: 'image',
      canVision: true,
    })
  })

  it('leaving chat clears reasoning and web search; blocks save while a rule fails', async () => {
    const chat: Model = {
      ...typed,
      id: 'claude',
      modelType: 'chat',
      nativeWebSearch: true,
      reasoning: { dialect: 'anthropic-budget', efforts: ['high'], budgetTokens: 2048 },
    }
    const anthropic: Endpoint = { ...endpoint, protocol: 'anthropic-messages', models: [chat] }
    const { saves } = setup({}, { ...stored, endpoints: [anthropic] })
    await fireEvent.click(await screen.findByRole('button', { name: 'Settings for claude' }))
    const dialog = within(await screen.findByRole('dialog', { name: 'Edit model' }))
    const save = dialog.getByRole('button', { name: 'Save' })
    await fireEvent.input(dialog.getByRole('spinbutton', { name: 'Maximum output (tokens)' }), {
      target: { value: '2048' },
    })
    expect(
      dialog.getByText('Budget must be at least 1024 and below max output (default 4096)'),
    ).toBeTruthy()
    expect(save.hasAttribute('disabled')).toBe(true)
    await fireEvent.click(dialog.getByRole('button', { name: 'Embedding' }))
    expect(dialog.queryByRole('button', { name: 'Reasoning' })).toBeNull()
    expect(save.hasAttribute('disabled')).toBe(false)
    await fireEvent.click(save)
    await waitFor(() => expect(saves).toHaveLength(1))
    expect(saves[0]?.endpoints[0]?.models[0]).toMatchObject({
      modelType: 'embedding',
      typeSource: 'user',
      reasoning: null,
      nativeWebSearch: false,
    })
  })

  it('names the model and the rule when the backend rejects a save', async () => {
    const save = vi.fn(() =>
      Promise.reject(
        new KitClientError({
          code: 'bad_request',
          message: 'invalid maxOutput',
          details: { field: 'maxOutput', reason: 'exceeds_context' },
        }),
      ),
    )
    const bad: Model = {
      ...typed,
      id: 'm1',
      name: 'Mine',
      modelType: 'chat',
      contextWindow: 100,
      maxOutput: 200,
    }
    const { notify } = setup({ save }, { ...stored, endpoints: [{ ...endpoint, models: [bad] }] })
    const url = await screen.findByRole('textbox', { name: 'API URL' })
    await fireEvent.input(url, { target: { value: 'https://other.example.com' } })
    await fireEvent.blur(url)
    await waitFor(() => expect(save).toHaveBeenCalledOnce())
    expect(
      (await screen.findByRole('alert')).textContent?.includes(
        'Settings for model Mine are invalid: Max output must be smaller than the context window',
      ),
    ).toBe(true)
    expect(notify.toast).not.toHaveBeenCalled()
  })

  it('a re-probe keeps user tools, name and group; new models take the probe group', async () => {
    const mine: Model = {
      ...typed,
      id: 'text-embedding-3',
      modelType: 'embedding',
      supportsTools: false,
      name: 'Mine',
      groupName: 'G',
    }
    const { saves } = setup({}, { ...stored, endpoints: [{ ...endpoint, models: [mine] }] })
    const picker = await openPicker()
    await fireEvent.click(picker.getByRole('button', { name: 'Add gpt-4o' }))
    await waitFor(() => expect(saves).toHaveLength(2))
    const models = saves.at(-1)?.endpoints[0]?.models ?? []
    expect(models.find((m) => m.id === 'text-embedding-3')).toMatchObject({
      supportsTools: false,
      name: 'Mine',
      groupName: 'G',
    })
    expect(models.find((m) => m.id === 'gpt-4o')).toMatchObject({ groupName: 'gpt', name: null })
  })

  it('blocks a reserved header with a message', async () => {
    const { saves } = setup()
    const dialog = await openConfig()
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Header name' }), {
      target: { value: 'Authorization' },
    })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    expect(dialog.getByRole('alert').textContent).toContain('Authorization is set by the system')
    expect(saves).toHaveLength(0)
  })

  it('round-trips header rows through JSON and blocks invalid JSON', async () => {
    const { saves } = setup()
    const dialog = await openConfig()
    await fireEvent.click(dialog.getByRole('button', { name: 'Switch to JSON editing' }))
    const text = dialog.getByRole('textbox', { name: 'Custom headers' }) as HTMLTextAreaElement
    expect(JSON.parse(text.value)).toEqual({ 'X-Team': 'a' })
    await fireEvent.input(text, { target: { value: '{"X-Team":"b","X-New":"c"}' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Switch to key-value list' }))
    const names = dialog.getAllByRole('textbox', { name: 'Header name' }) as HTMLInputElement[]
    expect(names.map((input) => input.value)).toEqual(['X-Team', 'X-New'])
    await fireEvent.click(dialog.getByRole('button', { name: 'Switch to JSON editing' }))
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Custom headers' }), {
      target: { value: '[1' },
    })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    expect(dialog.getByRole('alert').textContent).toBe('Invalid JSON')
    expect(saves).toHaveLength(0)
  })
})
