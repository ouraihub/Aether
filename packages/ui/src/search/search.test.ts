import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { expect, test, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, type SearchSave, type SearchSettingsDto } from '../ports'
import Harness from './Harness.test.svelte'

test('empty replacement preserves the key; clear is explicit and undo restores editing', async () => {
  const saves: SearchSave[] = []
  const settings = (capability: 'search' | 'fetch'): SearchSettingsDto => ({
    capability,
    defaultProvider: 'jina',
    ...(capability === 'search' ? { maxResults: 5 } : {}),
    providers: [
      {
        id: 'jina',
        name: 'Jina',
        description: 'Reader',
        keyMode: 'optional',
        basicAuth: false,
        ready: true,
        apiKeyMasked: 'stored-mask',
      },
    ],
  })
  const notify = { toast: vi.fn(), confirm: async () => true }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const ports = createFakePorts()
  ports.search = {
    settings: async (capability) => settings(capability),
    save: async (input, options) => {
      if (options?.signal?.aborted) throw new Error('Cannot save through an aborted settings session')
      saves.push(input)
      return settings(input.capability)
    },
    setDefault: async (capability) => settings(capability),
    reveal: async () => 'stored-secret',
    check: async () => ({ count: 1 }),
    search: async (query) => ({ query, provider: 'jina', hits: [] }),
    fetch: async (url) => ({ title: 'Page', url, content: 'Body' }),
  }
  const mounted = render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } })
  await screen.findAllByText('stored-mask')
  const section = document.querySelector<HTMLElement>('section[aria-labelledby="search-search"]')
  if (!section) throw new Error('Search settings region missing')
  const card = within(section)
  await fireEvent.click(card.getByRole('button', { name: 'Replace' }))
  await fireEvent.input(card.getByLabelText('API key'), { target: { value: 'replacement' } })
  await fireEvent.input(card.getByLabelText('API key'), { target: { value: '' } })
  await fireEvent.click(card.getByRole('button', { name: 'Save' }))
  await waitFor(() => expect(saves[0]).toEqual({ capability: 'search', provider: 'jina' }))
  await fireEvent.click(card.getByRole('button', { name: 'Clear' }))
  await fireEvent.click(card.getByRole('button', { name: 'Replace' }))
  expect(card.getByLabelText('API key').tagName).toBe('INPUT')
  await fireEvent.click(card.getByRole('button', { name: 'Undo' }))
  await fireEvent.click(card.getByRole('button', { name: 'Clear' }))
  await fireEvent.click(card.getByRole('button', { name: 'Save' }))
  await waitFor(() => expect(saves[1]?.apiKey).toBe(''))
  mounted.unmount()
})
