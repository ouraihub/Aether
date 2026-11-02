import { describe, expect, it, vi } from 'vitest'
import { KitClientError } from '../ports'
import {
  accepted,
  base,
  conversation,
  pickedModel,
  required,
  setup,
  stored,
} from './test-support/run-session-fixture'

describe('RunSession inputs', () => {
  it('rejects reply actions for writing and omits graph fields from a normal writing run', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    const writing = { ...base, purpose: 'writing' }
    expect(await session.retry(writing, 'source')).toBe(false)
    expect(session.actionError).toBe('bad_request')
    expect(await session.translate(writing, 'source', 'French')).toBe(false)
    expect(runs).toHaveLength(0)
    const sent = session.send(writing, 'Polish this')
    expect(required(runs[0]).request.replyToId).toBeUndefined()
    expect(required(runs[0]).request.retryTurnId).toBeUndefined()
    expect(required(runs[0]).request.translateTurnId).toBeUndefined()
    required(runs[0]).accepted.resolve(accepted)
    await sent
  })

  it('uses the picked model for an unaccepted local reply without a source model snapshot', async () => {
    const { session, runs } = setup(false)
    const sent = session.send({ ...base, model: null }, 'Question')
    required(runs[0]).accepted.reject(
      new KitClientError({ code: 'upstream', message: 'Unavailable' }),
    )
    await sent
    const retried = session.retry(base, required(session.turns[0]).id)
    expect(required(runs[1]).request.model).toEqual(pickedModel)
    required(runs[1]).accepted.resolve({ runId: 'local', conversationId: null, turnId: null })
    await retried
  })

  it('rejects missing source and invalid language without changing original replies', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    expect(await session.retry(base, 'missing')).toBe(false)
    expect(session.actionError).toBe('not_found')
    expect(await session.translate(base, 'source', ' ')).toBe(false)
    expect(session.actionError).toBe('bad_request')
    expect(await session.translate(base, 'source', 'x'.repeat(81))).toBe(false)
    expect(session.turns).toHaveLength(1)
    expect(runs).toHaveLength(0)
  })

  it('shows sent images at once, omits them when empty and lets history replace them', async () => {
    const { session, runs, get } = setup()
    session.show('conversation', [stored()], 'source')
    void session.send(base, 'Plain')
    expect('images' in required(runs[0]).request).toBe(false)
    required(runs[0]).accepted.reject(new KitClientError({ code: 'network', message: 'offline' }))
    await vi.waitFor(() => expect(session.running).toBe(false))
    const sent = session.send(base, 'Look', false, [{ mime: 'image/png', data: 'AAAA' }])
    expect(required(runs[1]).request.images).toEqual([{ mime: 'image/png', data: 'AAAA' }])
    expect(session.turns.at(-1)?.images).toEqual([
      { mime: 'image/png', url: 'data:image/png;base64,AAAA' },
    ])
    const hosted = { mime: 'image/png', url: '/images/0' }
    get.mockResolvedValue(conversation([stored(), stored({ id: 'candidate', images: [hosted] })]))
    required(runs[1]).accepted.resolve(accepted)
    await sent
    required(runs[1]).emit({ kind: 'end', status: 'completed' })
    await vi.waitFor(() =>
      expect(session.turns.find((turn) => turn.id === 'candidate')?.images).toEqual([hosted]),
    )
  })

  it('retries a stored source by id only and an unstored one by resending its images', async () => {
    const { session, runs } = setup()
    session.show(
      'conversation',
      [stored({ images: [{ mime: 'image/png', url: '/images/0' }] })],
      'source',
    )
    void session.retry(base, 'source')
    expect(required(runs[0]).request).toMatchObject({ retryTurnId: 'source' })
    expect('images' in required(runs[0]).request).toBe(false)
    expect(session.turns[1]?.images).toEqual([{ mime: 'image/png', url: '/images/0' }])

    const local = setup(false)
    const sent = local.session.send(base, 'Look', false, [{ mime: 'image/gif', data: 'R0lG' }])
    required(local.runs[0]).accepted.reject(
      new KitClientError({ code: 'upstream', message: 'Unavailable' }),
    )
    await sent
    void local.session.retry(base, required(local.session.turns[0]).id)
    expect(required(local.runs[1]).request.images).toEqual([{ mime: 'image/gif', data: 'R0lG' }])
  })

  it('takes the streamed cost and sums known costs per currency', async () => {
    const { session, runs } = setup(false)
    session.show(
      null,
      [stored({ cost: { amount: 0.5, currency: 'USD' } }), stored({ id: 'b', cost: null })],
      'b',
    )
    void session.send(base, 'Priced')
    const usage = {
      inputTokens: 10,
      outputTokens: 2,
      cacheReadTokens: null,
      cacheWriteTokens: null,
    }
    required(runs[0]).emit({
      kind: 'usage',
      ...usage,
      reasoningTokens: null,
      cost: { amount: 1, currency: 'CNY' },
    })
    expect(session.turns.at(-1)?.cost).toEqual({ amount: 1, currency: 'CNY' })
    session.turns.push({
      ...required(session.turns.at(-1)),
      id: 'c',
      cost: { amount: 0.25, currency: 'USD' },
    })
    expect(session.cumulativeCost).toEqual([
      { amount: 0.75, currency: 'USD' },
      { amount: 1, currency: 'CNY' },
    ])
  })
})
