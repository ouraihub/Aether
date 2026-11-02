import { screen, within } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import type { Model, Provider } from '../ports'
import { endpoint, model, setup, stored } from './test-support/providers-fixture'
import { settingsTarget } from './revision.svelte'

describe('opening settings from chat (docs/28 RT2)', () => {
  it('selects the owning provider and endpoint and opens that model once', async () => {
    const target: Model = {
      ...model,
      id: 'claude-x',
      modelType: 'chat',
      typeSource: 'inferred',
      reasoning: null,
    }
    const other: Provider = {
      ...stored,
      id: 'p2',
      name: 'Relay',
      endpoints: [
        { ...endpoint, id: 'e2', slug: 'first' },
        { ...endpoint, id: 'e3', slug: 'second', protocol: 'anthropic-messages', models: [target] },
      ],
    }
    settingsTarget.value = { endpointId: 'e3', modelId: 'claude-x' }
    setup({ list: () => Promise.resolve([stored, other]) })
    const dialog = await screen.findByRole('dialog', { name: 'Edit model' })
    expect((within(dialog).getByLabelText('Model ID') as HTMLInputElement).value).toBe('claude-x')
    expect(settingsTarget.value).toBeNull()
  })

  it('drops a target whose endpoint no longer exists', async () => {
    settingsTarget.value = { endpointId: 'gone', modelId: 'x' }
    setup()
    expect((await screen.findAllByText('Local')).length).toBeGreaterThan(0)
    expect(settingsTarget.value).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
  })
})
