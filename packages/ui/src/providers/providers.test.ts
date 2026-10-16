import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, type Endpoint, type Provider, type ProviderSave } from '../ports'
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

function setup() {
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
    },
  })
  const notify = { toast: vi.fn(), confirm: () => Promise.resolve(true) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } })
  return saves
}

describe('ProvidersSettings', () => {
  it('saves picked models with the endpoint id and without touching the key', async () => {
    const saves = setup()
    await fireEvent.click(await screen.findByRole('button', { name: 'Fetch models' }))
    await screen.findByRole('dialog', { name: 'Pick models' })
    await fireEvent.click(screen.getByRole('button', { name: 'Add gpt-4o' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Use 1 models' }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves).toHaveLength(1))
    const saved = saves[0]?.endpoints[0]
    expect(saved?.id).toBe('e1')
    expect(saved && 'apiKey' in saved).toBe(false)
    expect(saved?.models.map((m) => m.id)).toEqual(['gpt-4o'])
  })

  it('bulk changes only search results and preserves picks outside the filter', async () => {
    const saves = setup()
    await fireEvent.click(await screen.findByRole('button', { name: 'Fetch models' }))
    await screen.findByRole('dialog', { name: 'Pick models' })
    await fireEvent.click(screen.getByRole('button', { name: 'Add gpt-4o' }))
    await fireEvent.input(screen.getByRole('textbox', { name: 'Search model ID' }), { target: { value: 'embedding' } })
    await fireEvent.click(screen.getByRole('button', { name: 'Add all' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Remove all' }))
    await fireEvent.input(screen.getByRole('textbox', { name: 'Search model ID' }), { target: { value: '' } })
    expect(screen.getByRole('button', { name: 'Remove gpt-4o' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Add text-embedding-3' })).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: 'Use 1 models' }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Save' }))
    await waitFor(() => expect(saves[0]?.endpoints[0]?.models.map((m) => m.id)).toEqual(['gpt-4o']))
  })
})
