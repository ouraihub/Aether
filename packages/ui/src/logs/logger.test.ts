import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createFakePorts } from '../ports'
import type { IngestBatch } from '../ports'
import { createLogger } from './logger'

function setup(
  ingest: (batch: IngestBatch) => Promise<{ accepted: number; rejected: number }> = (b) =>
    Promise.resolve({ accepted: b.entries.length, rejected: 0 }),
) {
  const batches: IngestBatch[] = []
  const logs = createFakePorts({
    logs: {
      ingest: (batch) => {
        batches.push(structuredClone(batch))
        return ingest(batch)
      },
    },
  }).logs
  return { batches, logger: createLogger(logs) }
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

describe('createLogger', () => {
  it('sends after 250 ms, or at once when 20 entries are queued', async () => {
    const { batches, logger } = setup()
    logger.info('ui.panel.opened', { count: 1, ok: true, name: 'x' })
    await vi.advanceTimersByTimeAsync(249)
    expect(batches).toEqual([])
    await vi.advanceTimersByTimeAsync(1)
    expect(batches).toEqual([
      {
        entries: [{ level: 'info', event: 'ui.panel.opened', fields: { count: 1, ok: true, name: 'x' } }],
        droppedCount: 0,
      },
    ])
    for (let i = 0; i < 20; i++) logger.debug('ui.tick')
    await vi.advanceTimersByTimeAsync(0)
    expect(batches[1]?.entries).toHaveLength(20)
  })

  it('drops entries that break the whitelist and reports them as droppedCount', async () => {
    const { batches, logger } = setup()
    logger.info('Bad Event')
    logger.info('single')
    logger.info('ui.x', { 'Bad-Key': 1 })
    logger.info('ui.x', { text: 'a'.repeat(201) })
    logger.info('ui.x', { n: Number.NaN })
    logger.warn('ui.ok')
    await logger.flush()
    expect(batches).toEqual([{ entries: [{ level: 'warn', event: 'ui.ok' }], droppedCount: 5 }])
  })

  it('keeps warn/error over debug/info when 200 entries are already queued', async () => {
    const gate = Promise.withResolvers<void>()
    let calls = 0
    const { batches, logger } = setup(async (b) => {
      if (calls++ === 0) await gate.promise // the first request stays in flight while the rest queues up
      return { accepted: b.entries.length, rejected: 0 }
    })
    logger.error('ui.first')
    void logger.flush()
    logger.warn('ui.warn')
    for (let i = 0; i < 199; i++) logger.info('ui.info')
    logger.error('ui.last') // queue full: evicts the oldest info
    logger.debug('ui.extra') // evicts another info
    gate.resolve()
    await vi.runAllTimersAsync()
    const sent = batches.slice(1).flatMap((b) => b.entries)
    expect(sent).toHaveLength(200)
    expect(sent[0]).toEqual({ level: 'warn', event: 'ui.warn' })
    expect(sent.filter((e) => e.event === 'ui.info')).toHaveLength(197)
    expect(sent.at(-2)).toEqual({ level: 'error', event: 'ui.last' })
    expect(batches.map((b) => b.droppedCount)).toEqual([0, 2, 0, 0, 0])
  })

  it('flushes on pagehide', async () => {
    const { batches, logger } = setup()
    logger.error('ui.crash')
    window.dispatchEvent(new Event('pagehide'))
    await vi.advanceTimersByTimeAsync(0)
    expect(batches).toHaveLength(1)
  })
})
