import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import type { Provider, ProviderSave } from '../ports'
import { endpoint, setup, stored } from './test-support/providers-fixture'

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
      Promise.resolve(
        input.presetId === 'fenno' ? [{ ...stored, presetId: 'qiniu' }, seeded] : [stored],
      ),
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
      endpoints: [
        { slug: 'ep-1', baseUrl: 'https://api.fenno.ai/v1', protocol: 'openai-chat', models: [] },
      ],
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
