import { describe, expect, it, vi } from 'vitest'
import { KitClientError } from '../ports'
import type { Conversation } from '../ports'
import { accepted, base, conversation, deferred, required, setup, stored } from './test-support/run-session-fixture'

describe('RunSession lifecycle', () => {
  it('waits for acceptance after an early end, then merges stored text and metadata without losing tools/search', async () => {
    const { session, runs, get } = setup()
    const source = stored()
    const candidate = stored({
      id: 'candidate',
      questionId: 'candidate',
      parentId: source.id,
      text: 'Persisted answer',
    })
    session.show('conversation', [source], source.id)
    get.mockResolvedValue(conversation([source, candidate], candidate.id))
    const sent = session.send(base, 'Next question', true)
    const run = required(runs[0])
    run.emit({ kind: 'text_delta', text: 'Streamed answer' })
    run.emit({ kind: 'tool_call', callId: 'tool', name: 'read_note', source: 'host' })
    run.emit({ kind: 'tool_result', callId: 'tool', outcome: 'ok', summary: 'Read note' })
    const hits = [{ title: 'Source', url: 'https://example.com', snippet: 'Excerpt' }]
    run.emit({ kind: 'search_result', query: 'Next question', provider: 'search-provider', hits })
    run.emit({ kind: 'end', status: 'completed', exhausted: true })
    expect(get).not.toHaveBeenCalled()
    expect(session.running).toBe(true)
    run.accepted.resolve(accepted)
    await sent
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(get).toHaveBeenCalledOnce()
    expect(get).toHaveBeenCalledWith('conversation')
    expect(session.turns[1]).toMatchObject({
      id: candidate.id,
      text: candidate.text,
      questionId: candidate.questionId,
      parentId: candidate.parentId,
      exhausted: true,
      tools: [{ callId: 'tool', summary: 'Read note', outcome: 'ok' }],
      search: { status: 'complete', hits },
    })
    expect(session.selectedTurnId).toBe(candidate.id)
    session.show('conversation', [source, candidate], source.id)
    expect(required(required(session.turns[1]).tools[0]).summary).toBe('Read note')
    expect(required(session.turns[1]).search.hits).toEqual(hits)
    session.show(
      'conversation',
      [source, { ...candidate, text: 'Edited answer', reasoningText: null, contextUsage: null }],
      source.id,
    )
    expect(required(session.turns[1]).tools).toEqual([])
    expect(required(session.turns[1]).search.status).toBe('idle')
  })


  it('retains originals and exposes both acceptance rejection and streamed failure', async () => {
    const { session, runs, get } = setup()
    const source = stored()
    session.show('conversation', [source], source.id)
    const retry = session.retry(base, source.id)
    required(runs[0]).accepted.reject(
      new KitClientError({ code: 'needs_config', message: 'Source model removed' }),
    )
    expect(await retry).toBe(false)
    expect(session.actionError).toBe('needs_config')
    expect(session.turns[0]).toMatchObject({
      id: source.id,
      text: source.text,
      questionId: source.questionId,
    })
    expect(session.turns).toHaveLength(1)
    expect(session.selectedTurnId).toBe(source.id)
    expect(get).not.toHaveBeenCalled()

    get.mockResolvedValue(conversation([source]))
    const translation = session.translate(base, source.id, 'French')
    expect(session.actionError).toBeNull()
    required(runs[1]).accepted.resolve(accepted)
    await translation
    required(runs[1]).emit({
      kind: 'end',
      status: 'failed',
      error: { code: 'upstream', message: 'Provider failed' },
    })
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(session.actionError).toBeNull()
    expect(session.turns[1]).toMatchObject({ status: 'failed', error: 'upstream' })
    expect(session.turns[0]).toMatchObject({
      id: source.id,
      text: source.text,
      questionId: source.questionId,
    })
    expect(session.selectedTurnId).toBe(source.id)
  })


  it('removes a rejected translation candidate and retains its original', async () => {
    const { session, runs } = setup()
    const source = stored()
    session.show('conversation', [source], source.id)
    const translated = session.translate(base, source.id, 'French')
    required(runs[0]).accepted.reject(
      new KitClientError({ code: 'upstream', message: 'Provider unavailable' }),
    )
    expect(await translated).toBe(false)
    expect(session.actionError).toBe('upstream')
    expect(session.turns).toHaveLength(1)
    expect(required(session.turns[0]).text).toBe(source.text)
    expect(session.selectedTurnId).toBe(source.id)
  })


  it('keeps the streamed reply and exposes a failed persistence refresh', async () => {
    const { session, runs, get } = setup()
    session.show('conversation', [stored()], 'source')
    get.mockRejectedValue(new Error('History unavailable'))
    const retry = session.retry(base, 'source')
    required(runs[0]).accepted.resolve(accepted)
    await retry
    required(runs[0]).emit({ kind: 'text_delta', text: 'Still visible' })
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(session.turns[1]).toMatchObject({ id: 'candidate', text: 'Still visible' })
    expect(session.actionError).toBeNull()
    expect(session.historyRefreshError).toBe('History unavailable')
  })


  it('retains a rejected normal reply locally but never marks its temporary id persisted', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    const sent = session.send(base, 'New question')
    required(runs[0]).accepted.reject(
      new KitClientError({ code: 'upstream', message: 'Unavailable' }),
    )
    expect(await sent).toBe(false)
    expect(session.turns[1]).toMatchObject({ persisted: false, status: 'failed' })
    expect(session.selectedTurnId).toBe('source')
    expect(session.actionError).toBeNull()
    const localId = required(session.turns[1]).id
    const retry = session.retry(base, localId)
    expect(required(runs[1]).request).toMatchObject({
      replyToId: 'source',
      input: 'New question',
      regenerate: false,
    })
    expect(required(runs[1]).request.retryTurnId).toBeUndefined()
    required(runs[1]).accepted.resolve(accepted)
    await retry
    expect(session.turns[2]).toMatchObject({
      id: 'candidate',
      questionId: 'candidate',
      persisted: true,
    })
  })


  it('ignores delayed acceptance and history refresh when another conversation is shown', async () => {
    const { session, runs, get } = setup()
    const pendingHistory = deferred<Conversation>()
    get.mockReturnValue(pendingHistory.promise)
    session.show('conversation', [stored()], 'source')
    const sent = session.send(base, 'Next')
    required(runs[0]).accepted.resolve(accepted)
    await sent
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    session.show('other', [stored({ id: 'other-source' })], 'other-source')
    pendingHistory.resolve(conversation([stored()]))
    await pendingHistory.promise
    expect(session.conversationId).toBe('other')
    expect(required(session.turns[0]).id).toBe('other-source')

    const delayed = session.send(base, 'Delayed')
    session.show(null)
    required(runs[1]).accepted.resolve(accepted)
    await delayed
    expect(session.conversationId).toBeNull()
    expect(session.turns).toEqual([])
  })


  it('does not invent a terminal state when cancellation is acknowledged', async () => {
    const { session, runs, cancel } = setup(false)
    const sent = session.send(base, 'Partial')
    const run = required(runs[0])
    run.accepted.resolve({ runId: 'r', conversationId: null, turnId: null })
    await sent
    run.emit({ kind: 'text_delta', text: 'kept' })
    session.stop()
    await Promise.resolve()
    expect(cancel).toHaveBeenCalledWith('r')
    expect(session.running).toBe(true)
    expect(session.turns[0]).toMatchObject({ text: 'kept', status: 'running' })
    run.emit({ kind: 'end', status: 'cancelled' })
    expect(session.running).toBe(false)
    expect(session.turns[0]).toMatchObject({ text: 'kept', status: 'cancelled' })
  })


  it('cancels late acceptance and isolates old events after changing the displayed conversation', async () => {
    const { session, runs, cancel } = setup(false)
    const sent = session.send(base, 'Old')
    const old = required(runs[0])
    session.show('new', [stored({ id: 'new-turn', text: 'New' })])
    old.emit({ kind: 'text_delta', text: 'old text' })
    old.accepted.resolve({ runId: 'late', conversationId: 'old', turnId: 'old-turn' })
    await sent
    old.emit({ kind: 'end', status: 'completed' })
    expect(cancel).toHaveBeenCalledWith('late')
    expect(session.conversationId).toBe('new')
    expect(session.turns).toHaveLength(1)
    expect(session.turns[0]).toMatchObject({ id: 'new-turn', text: 'New' })
  })


  it('reports a current approval failure but does not revive a card after switching sessions', async () => {
    const { session, runs, approve } = setup(false)
    const failure = deferred<void>()
    approve.mockReturnValueOnce(failure.promise)
    const sent = session.send(base, 'Tool')
    const run = required(runs[0])
    run.accepted.resolve({ runId: 'r', conversationId: null, turnId: null })
    await sent
    run.emit({ kind: 'tool_call', callId: 'call', name: 'tool', source: 'host' })
    run.emit({ kind: 'approval_required', callId: 'call', name: 'tool', argsPreview: 'preview' })
    const tool = required(session.turns[0]?.tools[0])
    session.approve(tool, true)
    failure.reject(new KitClientError({ code: 'conflict', message: 'expired' }))
    await Promise.resolve()
    expect(session.actionError).toBe('conflict')
    expect(tool.pending).toBe('preview')
    const oldFailure = deferred<void>()
    approve.mockReturnValueOnce(oldFailure.promise)
    session.approve(tool, true)
    session.show(null)
    oldFailure.reject(new KitClientError({ code: 'not_found', message: 'old' }))
    await Promise.resolve()
    expect(session.turns).toEqual([])
    expect(session.actionError).toBeNull()
    expect(tool.pending).toBeNull()
  })
})
