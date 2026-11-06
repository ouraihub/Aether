import { describe, expect, it } from 'vitest'
import { createI18n } from '../i18n'
import type { ModelOption, Provider } from '../ports'
import {
  effortBars,
  groupModels,
  groupSummary,
  nextValue,
  parseQuery,
  pickerModels,
  score,
  type PickerModel,
} from './model-effort'

const zh = createI18n('zh-CN')
const model = (over: Partial<PickerModel> = {}): PickerModel => ({
  key: 'e1\nopus',
  providerId: 'p1',
  provider: 'Provider',
  name: 'Opus',
  detail: 'main',
  hasKey: false,
  efforts: ['none', 'low', 'high'],
  contextWindow: null,
  ...over,
})

describe('picker search', () => {
  it('splits only a known trailing level and maps off to none', () => {
    expect(parseQuery('  opus HIGH ')).toEqual({ text: 'opus', effort: 'high' })
    expect(parseQuery('opus off')).toEqual({ text: 'opus', effort: 'none' })
    expect(parseQuery('high')).toEqual({ text: 'high', effort: null })
    expect(parseQuery('opus mystery')).toEqual({ text: 'opus mystery', effort: null })
  })

  it('hides models without the typed level and ranks substring before fuzzy', () => {
    const opus = model()
    expect(score(opus, 'opus high')).not.toBeNull()
    expect(score(opus, 'opus max')).toBeNull()
    expect(score(opus, 'provider opus')).toBe(500)
    expect(score(opus, 'opus mystery')).toBeNull()
    const exact = score(opus, 'opus') ?? Infinity
    const fuzzy = score(opus, 'ops') ?? Infinity
    expect(exact).toBeLessThan(fuzzy)
  })

  it('ranks matches inside groups and drops empty groups while searching', () => {
    const models = [
      model({ key: 'a', name: 'zeta opus' }),
      model({ key: 'b', name: 'opus' }),
      model({ key: 'c', name: 'other', providerId: 'p2', provider: 'Other' }),
    ]
    const groups = groupModels(models, 'opus')
    expect(groups.map((group) => group.id)).toEqual(['p1'])
    expect(groups[0]?.models.map((item) => item.key)).toEqual(['b', 'a'])
    expect(groupModels(models, '')).toHaveLength(2)
  })
})

describe('picker selection', () => {
  it('keeps a declared explicit level but never picks the first level itself', () => {
    const opus = model()
    expect(nextValue(opus, null, '').effort).toBe('default')
    expect(nextValue(opus, { key: 'x', effort: 'high' }, '').effort).toBe('high')
    expect(nextValue(opus, { key: 'x', effort: 'max' }, '').effort).toBe('default')
    expect(nextValue(opus, { key: 'x', effort: 'high' }, 'opus off').effort).toBe('none')
  })

  it('lights bars by rank among reasoning levels, ignoring none and default', () => {
    const opus = model({ efforts: ['none', 'minimal', 'low', 'medium', 'high'] })
    expect(effortBars(opus, 'default')).toBe(0)
    expect(effortBars(opus, 'none')).toBe(0)
    expect(effortBars(opus, 'low')).toBe(2)
    expect(effortBars(opus, 'high')).toBe(4)
  })
})

describe('picker rows', () => {
  const option: ModelOption = {
    endpointId: 'e1',
    modelId: 'gpt-6.1-sol',
    label: 'Fenno · main · gpt-6.1-sol',
    name: null,
    modelType: 'chat',
    supportsTools: true,
    reasoning: { dialect: 'openai', efforts: ['high', 'none', 'low'] },
    nativeWebSearch: false,
    canVision: false,
  }
  const provider = {
    id: 'p1',
    name: 'Fenno',
    endpoints: [
      {
        id: 'e1',
        slug: '默认线路',
        keys: [{ enabled: true, readable: true }],
        models: [
          {
            id: 'gpt-6.1-sol',
            contextWindow: 1_100_000,
            pricing: {
              currency: 'USD',
              tiers: [
                { minInputTokens: 0, input: 2, output: 10, cacheRead: null, cacheWrite: null },
              ],
            },
          },
        ],
      },
    ],
  } as unknown as Provider

  it('joins route, first price tier and context window without guessing', () => {
    const [row] = pickerModels([option], [provider], zh)
    expect(row?.detail).toBe('默认线路 · USD 2 / 10 每百万 token 起 · 1.1M 上下文')
    expect(row?.provider).toBe('Fenno')
    expect(row?.hasKey).toBe(true)
    expect(row?.efforts).toEqual(['none', 'low', 'high'])
    expect(groupSummary({ id: 'p1', provider: 'Fenno', models: row ? [row] : [] }, zh)).toBe(
      'API 密钥 · 默认线路 1',
    )
  })

  it('falls back to the option label when provider metadata is unavailable', () => {
    const [row] = pickerModels([option], null, zh)
    expect(row?.detail).toBe('main')
    expect(row?.provider).toBe('Fenno')
    expect(row?.hasKey).toBe(false)
    expect(row?.contextWindow).toBeNull()
  })
})
