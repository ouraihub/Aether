import { render } from '@testing-library/svelte'
import { vi } from 'vitest'
import { createI18n } from '../../i18n'
import {
  createFakePorts,
  type Endpoint,
  type Provider,
  type ProviderPreset,
  type ProviderSave,
} from '../../ports'
import Harness from '../Harness.test.svelte'

export const endpoint: Endpoint = {
  id: 'e1',
  slug: 'main',
  baseUrl: 'http://127.0.0.1:9/v1',
  protocol: 'openai-chat',
  keys: [
    {
      id: 'k1',
      label: '',
      enabled: true,
      masked: 'sk-a****wxyz',
      readable: true,
      exhaustedUntil: null,
      exhaustedReason: null,
    },
  ],
  keyStrategy: 'round_robin',
  probeUrl: 'http://127.0.0.1:9/v1/models',
  imageBaseUrl: null,
  imageEditBaseUrl: null,
  headers: { 'X-Team': 'a' },
  models: [],
}

export const stored: Provider = {
  id: 'p1',
  name: 'Local',
  enabled: true,
  notes: '',
  presetId: null,
  endpoints: [endpoint],
}

export const twoKeys: Endpoint = {
  ...endpoint,
  keys: [
    ...endpoint.keys,
    {
      id: 'k2',
      label: 'backup',
      enabled: true,
      masked: 'sk-b****1234',
      readable: true,
      exhaustedUntil: null,
      exhaustedReason: null,
    },
  ],
}

export const model = {
  name: null,
  groupName: null,
  pricing: null,
  contextWindow: null,
  maxOutput: null,
  canVision: false,
  supportsTools: null,
  nativeWebSearch: false,
} as const

export function setup(
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
          suggestedProtocol: null,
          models: [
            {
              ...model,
              id: 'gpt-4o',
              group: 'gpt',
              modelType: 'chat',
              typeSource: 'inferred',
              reasoning: null,
            },
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
  render(Harness, {
    kit: { ports, i18n: createI18n('en'), notify, logger },
    ...(presets ? { presets } : {}),
  })
  return { saves, notify }
}
