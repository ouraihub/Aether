import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, type Endpoint, type Provider, type ProviderSave } from '../ports'
import { adopt, savable, toDraft } from './draft'
import Harness from './Harness.test.svelte'

const endpoint: Endpoint = {
  id: 'e1',
  slug: 'main',
  baseUrl: 'http://127.0.0.1:9/v1',
  protocol: 'openai-chat',
  hasKey: true,
  keyMasked: 'sk-a…wxyz',
  keyReadable: true,
  probeUrl: 'http://127.0.0.1:9/v1/models',
  models: [],
}
const stored: Provider = { id: 'p1', name: 'Local', enabled: true, endpoints: [endpoint] }
const model = {
  contextWindow: null,
  maxOutput: null,
  canVision: false,
  supportsTools: null,
  nativeWebSearch: false,
} as const

function setup(overrides: NonNullable<Parameters<typeof createFakePorts>[0]>['providers'] = {}) {
  const saves: ProviderSave[] = []
  const ports = createFakePorts({
    providers: {
      list: () => Promise.resolve([stored]),
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
  render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } })
  return { saves, notify }
}

async function openPicker() {
  await fireEvent.click(await screen.findByRole('button', { name: 'Sync models' }))
  return within(await screen.findByRole('dialog', { name: 'Pick models' }))
}

describe('ProvidersSettings', () => {
  it('saves each +/- at once with the endpoint id and without touching the key', async () => {
    const { saves } = setup()
    const picker = await openPicker()
    await fireEvent.click(picker.getByRole('button', { name: 'Add gpt-4o' }))
    await waitFor(() => expect(saves).toHaveLength(1))
    expect(screen.queryByRole('button', { name: /Use \d+ models/ })).toBeNull()
    const saved = saves[0]?.endpoints[0]
    expect(saved?.id).toBe('e1')
    expect(saved && 'apiKey' in saved).toBe(false)
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
    expect(revealKey).toHaveBeenCalledWith('e1', '')
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
    expect(saves[0]?.endpoints[0] && 'apiKey' in saves[0].endpoints[0]).toBe(false)
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
})

describe('draft helpers', () => {
  it('drops unsaved endpoints without a usable address and adopts stored ids by slug', () => {
    const draft = toDraft(stored)
    const added = { saved: null, slug: 'ep-2', baseUrl: '', protocol: 'openai-chat' as const, key: 'k', models: [] }
    draft.endpoints.push(added)
    expect(savable(draft).endpoints.map((e) => e.slug)).toEqual(['main'])

    added.baseUrl = 'https://b.example.com'
    const sent = structuredClone(savable(draft))
    const created: Endpoint = { ...endpoint, id: 'e2', slug: 'ep-2', baseUrl: 'https://b.example.com' }
    const next = adopt(draft, sent, { ...stored, endpoints: [endpoint, created] })
    expect(next.endpoints[1]?.saved?.id).toBe('e2')
    expect(next.endpoints[1]?.key).toBeNull()

    added.key = 'typed-after-send'
    expect(adopt(draft, sent, { ...stored, endpoints: [endpoint, created] }).endpoints[1]?.key).toBe('typed-after-send')
  })
})
