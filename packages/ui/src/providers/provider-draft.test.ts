import { describe, expect, it } from 'vitest'
import type { Endpoint, Model, ModelPricing, ReasoningConfig } from '../ports'
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
import { endpoint, model, stored } from './test-support/providers-fixture'

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
    const created: Endpoint = {
      ...endpoint,
      id: 'e2',
      slug: 'ep-2',
      baseUrl: 'https://b.example.com',
      keys: [],
    }
    const next = adopt(draft, sent, { ...stored, endpoints: [endpoint, created] })
    expect(next.endpoints[1]?.saved?.id).toBe('e2')
  })

  it('picks the first enabled key as primary', () => {
    const k1 = {
      id: 'k1',
      label: '',
      enabled: false,
      masked: 'sk-a****wxyz',
      readable: true,
      exhaustedUntil: null,
      exhaustedReason: null,
    }
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
    expect(checkHeaders([{ name: 'bad name', value: '1' }])).toEqual({
      error: { key: 'headers_invalid' },
    })
    expect(checkHeaders([{ name: 'X-A', value: 'a\nb' }])).toEqual({
      error: { key: 'headers_invalid' },
    })
    const many = Array.from({ length: 21 }, (_, i) => ({ name: `X-${i}`, value: '' }))
    expect(checkHeaders(many)).toEqual({ error: { key: 'headers_too_many' } })
  })

  const chat: Model = {
    ...model,
    id: 'm',
    modelType: 'chat',
    typeSource: 'inferred',
    reasoning: null,
  }

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
      [
        { reasoning: { dialect: 'openai', efforts: [] } },
        'openai-chat',
        ['reasoning:invalid_efforts'],
      ],
      [
        { reasoning: { dialect: 'openai', efforts: ['low', 'low'] } },
        'openai-chat',
        ['reasoning:invalid_efforts'],
      ],
      [
        { reasoning: { dialect: 'openai', efforts: ['default'] } },
        'openai-chat',
        ['reasoning:invalid_efforts'],
      ],
      [
        { modelType: 'image', reasoning: { dialect: 'openai', efforts: ['low'] } },
        'openai-chat',
        ['reasoning:invalid_efforts'],
      ],
      [
        { reasoning: { dialect: 'openai', efforts: ['low'] } },
        'anthropic-messages',
        ['reasoning:protocol_mismatch'],
      ],
      [{ reasoning: budget(2048) }, 'openai-responses', ['reasoning:protocol_mismatch']],
      [{ reasoning: budget() }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [{ reasoning: budget(1023) }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [{ reasoning: budget(4096) }, 'anthropic-messages', ['reasoning:invalid_budget']],
      [{ reasoning: budget(4095) }, 'anthropic-messages', []],
      [
        { maxOutput: 2048, reasoning: budget(2048) },
        'anthropic-messages',
        ['reasoning:invalid_budget'],
      ],
      [
        { reasoning: { dialect: 'anthropic-adaptive', efforts: ['high'], budgetTokens: 2048 } },
        'anthropic-messages',
        ['reasoning:unexpected_budget'],
      ],
      [{ pricing: { currency: 'USD', tiers: [tier] } }, 'openai-chat', []],
      [
        { modelType: 'image', pricing: { currency: 'USD', tiers: [tier] } },
        'openai-chat',
        ['pricing:invalid'],
      ],
      [{ pricing: { currency: 'USD', tiers: [] } }, 'openai-chat', ['pricing:invalid']],
      [
        { pricing: { currency: 'USD', tiers: Array(9).fill(tier) } },
        'openai-chat',
        ['pricing:invalid'],
      ],
      [
        { pricing: { currency: 'CNY', tiers: [{ ...tier, minInputTokens: 1 }] } },
        'openai-chat',
        ['pricing:invalid'],
      ],
      [{ pricing: { currency: 'USD', tiers: [tier, tier] } }, 'openai-chat', ['pricing:invalid']],
      [
        { pricing: { currency: 'USD', tiers: [{ ...tier, input: -1 }] } },
        'openai-chat',
        ['pricing:invalid'],
      ],
      [
        { pricing: { currency: 'USD', tiers: [{ ...tier, output: NaN }] } },
        'openai-chat',
        ['pricing:invalid'],
      ],
      [
        { pricing: { currency: 'USD', tiers: [{ ...tier, cacheRead: 1e6 + 1 }] } },
        'openai-chat',
        ['pricing:invalid'],
      ],
    ]
    for (const [patch, protocol, expected] of cases)
      expect(
        validateModel({ ...chat, ...patch }, protocol as Endpoint['protocol']).map(
          (e) => `${e.field}:${e.reason}`,
        ),
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
    expect(withPurpose(withPurpose(rich, 'edit'), 'chat')).toMatchObject({
      modelType: 'chat',
      canVision: true,
    })
  })

  it('toSave omits reasoning only when unchanged from the stored model or null on a new one (contract §2)', () => {
    const reasoning: ReasoningConfig = { dialect: 'openai', efforts: ['low', 'high'] }
    const saved = { ...chat, reasoning }
    const draft = toDraft({ ...stored, endpoints: [{ ...endpoint, models: [saved] }] })
    const first = draft.endpoints[0]
    if (!first) throw new Error('missing endpoint')
    const models = (list: Model[]) =>
      toSave({ ...draft, endpoints: [{ ...first, models: list }] }).endpoints[0]?.models
    expect(models([saved])?.[0]).not.toHaveProperty('reasoning')
    expect(models([{ ...saved, reasoning: { ...reasoning } }])?.[0]).not.toHaveProperty('reasoning')
    expect(models([{ ...saved, reasoning: null }])?.[0]).toHaveProperty('reasoning', null)
    const changed: ReasoningConfig = { dialect: 'openai', efforts: ['high'] }
    expect(models([{ ...saved, reasoning: changed }])?.[0]).toHaveProperty('reasoning', changed)
    expect(models([{ ...chat, id: 'new' }])?.[0]).not.toHaveProperty('reasoning')
    expect(models([{ ...chat, id: 'new', reasoning }])?.[0]).toHaveProperty('reasoning', reasoning)
  })
})
