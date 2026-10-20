import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  KitClientError,
  type Endpoint,
  type Model,
  type ModelPricing,
  type Provider,
  type ProviderPreset,
  type ProviderSave,
  type ReasoningConfig,
} from '../ports'
import {
  adopt,
  checkHeaders,
  newEndpoint,
  primaryKey,
  purposeOf,
  savable,
  toDraft,
  toSave,
  validateModel,
  withPurpose,
  withType,
} from './draft'
import Harness from './Harness.test.svelte'

const endpoint: Endpoint = {
  id: 'e1',
  slug: 'main',
  baseUrl: 'http://127.0.0.1:9/v1',
  protocol: 'openai-chat',
  keys: [{ id: 'k1', label: '', enabled: true, masked: 'sk-a****wxyz', readable: true }],
  probeUrl: 'http://127.0.0.1:9/v1/models',
  imageBaseUrl: null,
  imageEditBaseUrl: null,
  headers: { 'X-Team': 'a' },
  models: [],
}
const stored: Provider = { id: 'p1', name: 'Local', enabled: true, notes: '', presetId: null, endpoints: [endpoint] }
const twoKeys: Endpoint = {
  ...endpoint,
  keys: [...endpoint.keys, { id: 'k2', label: 'backup', enabled: true, masked: 'sk-b****1234', readable: true }],
}
const model = {
  name: null,
  groupName: null,
  pricing: null,
  contextWindow: null,
  maxOutput: null,
  canVision: false,
  supportsTools: null,
  nativeWebSearch: false,
} as const

function setup(
  overrides: NonNullable<Parameters<typeof createFakePorts>[0]>['providers'] = {},
  list = stored,
  presets?: ProviderPreset[],
) {
  const saves: ProviderSave[] = []
  const ports = createFakePorts({
    providers: {
      list: () => Promise.resolve([list]),
      save: (input) => {
        saves.push(input)
        return Promise.resolve([stored])
      },
      probe: () =>
        Promise.resolve({
          probeUrl: endpoint.probeUrl,
          error: null,
          models: [
            { ...model, id: 'gpt-4o', group: 'gpt', modelType: 'chat', typeSource: 'inferred', reasoning: null },
            {
              ...model,
              id: 'text-embedding-3',
              group: null,
              modelType: 'embedding',
              typeSource: 'inferred',
              reasoning: null,
            },
          ],
        }),
      ...overrides,
    },
  })
  const notify = { toast: vi.fn(), confirm: () => Promise.resolve(true) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger }, ...(presets ? { presets } : {}) })
  return { saves, notify }
}

describe('provider presets (docs/39)', () => {
  it('seeds only presets not yet in the list and links the selected one to its key page', async () => {
    const seeded: Provider = {
      ...stored,
      id: 'p2',
      name: 'Fenno',
      enabled: false,
      presetId: 'fenno',
      endpoints: [{ ...endpoint, id: 'e2', keys: [] }],
    }
    const save = vi.fn((input: ProviderSave) =>
      Promise.resolve(input.presetId === 'fenno' ? [{ ...stored, presetId: 'qiniu' }, seeded] : [stored]),
    )
    setup({ save }, { ...stored, presetId: 'qiniu' }, [
      { id: 'qiniu', name: 'Qiniu', baseUrl: 'https://api.qnaigc.com', protocol: 'openai-chat' },
      {
        id: 'fenno',
        name: 'Fenno',
        baseUrl: 'https://api.fenno.ai/v1',
        protocol: 'openai-chat',
        apiKeyUrl: 'https://api.fenno.ai/s/vWbM',
      },
    ])
    await waitFor(() => expect(save).toHaveBeenCalledOnce())
    expect(save).toHaveBeenCalledWith({
      name: 'Fenno',
      presetId: 'fenno',
      enabled: false,
      endpoints: [{ slug: 'ep-1', baseUrl: 'https://api.fenno.ai/v1', protocol: 'openai-chat', models: [] }],
    })
    expect(screen.queryByRole('link', { name: 'Get a key' })).toBeNull()
    await fireEvent.click(await screen.findByText('Fenno'))
    expect((await screen.findByRole('link', { name: 'Get a key' })).getAttribute('href')).toBe(
      'https://api.fenno.ai/s/vWbM',
    )
    await fireEvent.click(screen.getByRole('button', { name: 'More actions for Fenno' }))
    expect(await screen.findByRole('menuitem', { name: 'Model notes' })).toBeTruthy()
    expect(screen.queryByRole('menuitem', { name: 'Delete' })).toBeNull()
  })
})

async function openPicker() {
  await fireEvent.click(await screen.findByRole('button', { name: 'Sync models' }))
  return within(await screen.findByRole('dialog', { name: 'Pick models' }))
}

describe('ProvidersSettings', () => {
  it('saves each +/- at once with the endpoint id and ', async () => {
    const { saves } = setup()
    const picker = await openPicker()
    await fireEvent.click(picker.getByRole('button', { name: 'Add gpt-4o' }))
    await waitFor(() => expect(saves).toHaveLength(1))
    expect(screen.queryByRole('button', { name: /Use \d+ models/ })).toBeNull()
    const saved = saves[0]?.endpoints[0]
    expect(saved?.id).toBe('e1')
    expect(saved?.models.map((m) => m.id)).toEqual(['gpt-4o'])
  })

  it('bulk changes only search results and preserves picks outside the filter', async () => {
    const { saves } = setup()
    const picker = await openPicker()
    await fireEvent.click(picker.getByRole('button', { name: 'Add gpt-4o' }))
    await fireEvent.input(picker.getByRole('textbox', { name: 'Search model ID' }), { target: { value: 'embedding' } })
    await fireEvent.click(picker.getByRole('button', { name: 'Add all' }))
    await fireEvent.click(picker.getByRole('button', { name: 'Remove all' }))
    await fireEvent.input(picker.getByRole('textbox', { name: 'Search model ID' }), { target: { value: '' } })
    expect(picker.getByRole('button', { name: 'Remove gpt-4o' })).toBeTruthy()
    expect(picker.getByRole('button', { name: 'Add text-embedding-3' })).toBeTruthy()
    await waitFor(() => expect(saves.at(-1)?.endpoints[0]?.models.map((m) => m.id)).toEqual(['gpt-4o']))
    await fireEvent.click(picker.getByRole('button', { name: 'Remove gpt-4o' }))
    await waitFor(() => expect(saves.at(-1)?.endpoints[0]?.models).toEqual([]))
  })

  it('toggles enabled immediately without a whole-provider save', async () => {
    const setEnabled = vi.fn((_id: string, enabled: boolean) => Promise.resolve([{ ...stored, enabled }]))
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
    expect(screen.getByRole('textbox', { name: 'Search model platforms…' }).getAttribute('autocomplete')).toBe('off')
  })

  it('edits the stored key without browser autofill', async () => {
    const revealKey = vi.fn(() => Promise.resolve('sk-plain-key'))
    setup({ revealKey })
    await fireEvent.click(await screen.findByRole('button', { name: 'Manage API keys' }))
    const dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Edit key' }))
    await waitFor(() =>
      expect((document.querySelector('input[name="provider-api-key"]') as HTMLInputElement | null)?.value).toBe(
        'sk-plain-key',
      ),
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
    await fireEvent.click(within(url.parentElement as HTMLElement).getByRole('button', { name: 'Copy' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('https://copy.example.com/v1'))
  })

  it('row menu edits model notes and deletes the provider (Cherry context menu)', async () => {
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
    await waitFor(() => expect(saveKey).toHaveBeenCalledWith({ endpointId: 'e1', id: 'k2', enabled: false }))
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
    expect(saves[0]?.endpoints[0]).toMatchObject({ id: 'e1', baseUrl: 'https://api.example.com/v1' })
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
    await fireEvent.click(await screen.findByRole('button', { name: 'Configure API URL and custom headers' }))
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
    expect((dialog.getByRole('textbox', { name: 'Image generation base URL' }) as HTMLInputElement).value).toBe('')
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
      { ...typed, id: 'gpt-4o', modelType: 'chat', reasoning: { dialect: 'openai', efforts: ['low', 'high'] } },
    ]
    const { saves } = setup({}, { ...stored, endpoints: [{ ...endpoint, models }] })
    await fireEvent.click(await screen.findByRole('button', { name: 'Settings for gpt-4o' }))
    let dialog = within(await screen.findByRole('dialog', { name: 'Edit model' }))
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Model name' }), { target: { value: '  GPT 4o  ' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves).toHaveLength(1))
    const saved = saves[0]?.endpoints[0]?.models.find((m) => m.id === 'gpt-4o')
    expect(saved?.name).toBe('GPT 4o')
    expect(saved && 'reasoning' in saved).toBe(false)
    expect(await screen.findByText('GPT 4o')).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: 'Settings for gpt-image-1' }))
    dialog = within(await screen.findByRole('dialog', { name: 'Edit model' }))
    expect((dialog.getByRole('radio', { name: 'Image generation' }) as HTMLInputElement).checked).toBe(true)
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
    expect(dialog.getByText('Budget must be at least 1024 and below max output (default 4096)')).toBeTruthy()
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
    const bad: Model = { ...typed, id: 'm1', name: 'Mine', modelType: 'chat', contextWindow: 100, maxOutput: 200 }
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
    await waitFor(() => expect(saves).toHaveLength(1))
    const models = saves[0]?.endpoints[0]?.models ?? []
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
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Header name' }), { target: { value: 'Authorization' } })
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
    await fireEvent.input(dialog.getByRole('textbox', { name: 'Custom headers' }), { target: { value: '[1' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    expect(dialog.getByRole('alert').textContent).toBe('Invalid JSON')
    expect(saves).toHaveLength(0)
  })
})

describe('draft helpers', () => {
  it('keeps endpoints without an address, drops malformed new ones and adopts stored ids by slug', () => {
    const draft = toDraft(stored)
    const added = { ...newEndpoint(draft.endpoints), slug: 'ep-2' }
    draft.endpoints.push(added)
    expect(savable(draft).endpoints.map((e) => e.slug)).toEqual(['main', 'ep-2'])
    added.baseUrl = 'nope'
    expect(savable(draft).endpoints.map((e) => e.slug)).toEqual(['main'])

    added.baseUrl = 'https://b.example.com'
    const sent = structuredClone(savable(draft))
    const created: Endpoint = { ...endpoint, id: 'e2', slug: 'ep-2', baseUrl: 'https://b.example.com', keys: [] }
    const next = adopt(draft, sent, { ...stored, endpoints: [endpoint, created] })
    expect(next.endpoints[1]?.saved?.id).toBe('e2')
  })

  it('picks the first enabled key as primary', () => {
    const k1 = { id: 'k1', label: '', enabled: false, masked: 'sk-a****wxyz', readable: true }
    const k2 = { ...k1, id: 'k2', enabled: true }
    expect(primaryKey([k1, k2])?.id).toBe('k2')
    expect(primaryKey([k1])?.id).toBe('k1')
    expect(primaryKey([])).toBeNull()
  })

  it('mirrors the backend header rules', () => {
    expect(
      checkHeaders([
        { name: ' X-A ', value: '1' },
        { name: '', value: '' },
      ]),
    ).toEqual({ headers: { 'X-A': '1' } })
    expect(checkHeaders([{ name: 'x-api-key', value: 'k' }])).toEqual({
      error: { key: 'headers_reserved', name: 'x-api-key' },
    })
    expect(
      checkHeaders([
        { name: 'X-A', value: '1' },
        { name: 'x-a', value: '2' },
      ]),
    ).toEqual({ error: { key: 'headers_invalid' } })
    expect(checkHeaders([{ name: 'bad name', value: '1' }])).toEqual({ error: { key: 'headers_invalid' } })
    expect(checkHeaders([{ name: 'X-A', value: 'a\nb' }])).toEqual({ error: { key: 'headers_invalid' } })
    const many = Array.from({ length: 21 }, (_, i) => ({ name: `X-${i}`, value: '' }))
    expect(checkHeaders(many)).toEqual({ error: { key: 'headers_too_many' } })
  })

  const chat: Model = { ...model, id: 'm', modelType: 'chat', typeSource: 'inferred', reasoning: null }

  it('validateModel mirrors the backend rules and reason codes (contract §3)', () => {
    const budget = (budgetTokens?: number): ReasoningConfig => ({
      dialect: 'anthropic-budget',
      efforts: ['high'],
      ...(budgetTokens === undefined ? {} : { budgetTokens }),
    })
    const tier = { minInputTokens: 0, input: 1, output: 2, cacheRead: null, cacheWrite: null }
    const cases: [Partial<Model>, string, string[]][] = [
      [{}, 'openai-chat', []],
      [{ name: ' '.repeat(5) + 'x'.repeat(80) }, 'openai-chat', []],
      [{ name: 'x'.repeat(81) }, 'openai-chat', ['name:too_long']],
      [{ groupName: '字'.repeat(81) }, 'openai-chat', ['groupName:too_long']],
      [{ contextWindow: 0 }, 'openai-chat', ['contextWindow:invalid']],
      [{ maxOutput: 1.5 }, 'openai-chat', ['maxOutput:invalid']],
      [{ contextWindow: 100, maxOutput: 100 }, 'openai-chat', ['maxOutput:exceeds_context']],
      [{ modelType: 'image', contextWindow: 100, maxOutput: 100 }, 'openai-chat', []],
      [{ reasoning: { dialect: 'openai', efforts: [] } }, 'openai-chat', ['reasoning:invalid_efforts']],
      [{ reasoning: { dialect: 'openai', efforts: ['low', 'low'] } }, 'openai-chat', ['reasoning:invalid_efforts']],
      [{ reasoning: { dialect: 'openai', efforts: ['default'] } }, 'openai-chat', ['reasoning:invalid_efforts']],
      [
        { modelType: 'image', reasoning: { dialect: 'openai', efforts: ['low'] } },
        'openai-chat',
        ['reasoning:invalid_efforts'],
      ],
      [{ reasoning: { dialect: 'openai', efforts: ['low'] } }, 'anthropic-messages', ['reasoning:protocol_mismatch']],
      [{ reasoning: budget(2048) }, 'openai-responses', ['reasoning:protocol_mismatch']],
      [{ reasoning: budget() }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [{ reasoning: budget(1023) }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [{ reasoning: budget(4096) }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [{ reasoning: budget(4095) }, 'anthropic-messages', []],
      [{ maxOutput: 2048, reasoning: budget(2048) }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [
        { reasoning: { dialect: 'anthropic-adaptive', efforts: ['high'], budgetTokens: 2048 } },
        'anthropic-messages',
        ['reasoning:unexpected_budget'],
      ],
      [{ pricing: { currency: 'USD', tiers: [tier] } }, 'openai-chat', []],
      [{ modelType: 'image', pricing: { currency: 'USD', tiers: [tier] } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'USD', tiers: [] } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'USD', tiers: Array(9).fill(tier) } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'CNY', tiers: [{ ...tier, minInputTokens: 1 }] } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'USD', tiers: [tier, tier] } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'USD', tiers: [{ ...tier, input: -1 }] } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'USD', tiers: [{ ...tier, output: NaN }] } }, 'openai-chat', ['pricing:invalid']],
      [{ pricing: { currency: 'USD', tiers: [{ ...tier, cacheRead: 1e6 + 1 }] } }, 'openai-chat', ['pricing:invalid']],
    ]
    for (const [patch, protocol, expected] of cases)
      expect(
        validateModel({ ...chat, ...patch }, protocol as Endpoint['protocol']).map((e) => `${e.field}:${e.reason}`),
        JSON.stringify(patch),
      ).toEqual(expected)
  })

  it('maps purpose to type and vision, and clears chat-only settings when leaving chat', () => {
    const reasoning: ReasoningConfig = { dialect: 'openai', efforts: ['low'] }
    const pricing: ModelPricing = { currency: 'USD', tiers: [] }
    const rich: Model = { ...chat, reasoning, nativeWebSearch: true, pricing, canVision: true }
    expect(purposeOf(rich)).toBe('chat')
    expect(withPurpose(rich, 'image')).toMatchObject({
      modelType: 'image',
      typeSource: 'user',
      canVision: false,
      reasoning: null,
      nativeWebSearch: false,
      pricing: null,
    })
    expect(purposeOf(withPurpose(rich, 'edit'))).toBe('edit')
    expect(purposeOf({ ...chat, modelType: 'embedding' })).toBeNull()
    expect(withType(rich, 'chat')).toBe(rich)
    expect(withPurpose(withPurpose(rich, 'edit'), 'chat')).toMatchObject({ modelType: 'chat', canVision: true })
  })

  it('toSave omits reasoning only when unchanged from the stored model or null on a new one (contract §2)', () => {
    const reasoning: ReasoningConfig = { dialect: 'openai', efforts: ['low', 'high'] }
    const saved = { ...chat, reasoning }
    const draft = toDraft({ ...stored, endpoints: [{ ...endpoint, models: [saved] }] })
    const first = draft.endpoints[0]
    if (!first) throw new Error('missing endpoint')
    const models = (list: Model[]) => toSave({ ...draft, endpoints: [{ ...first, models: list }] }).endpoints[0]?.models
    expect(models([saved])?.[0]).not.toHaveProperty('reasoning')
    expect(models([{ ...saved, reasoning: { ...reasoning } }])?.[0]).not.toHaveProperty('reasoning')
    expect(models([{ ...saved, reasoning: null }])?.[0]).toHaveProperty('reasoning', null)
    const changed: ReasoningConfig = { dialect: 'openai', efforts: ['high'] }
    expect(models([{ ...saved, reasoning: changed }])?.[0]).toHaveProperty('reasoning', changed)
    expect(models([{ ...chat, id: 'new' }])?.[0]).not.toHaveProperty('reasoning')
    expect(models([{ ...chat, id: 'new', reasoning }])?.[0]).toHaveProperty('reasoning', reasoning)
  })
})
