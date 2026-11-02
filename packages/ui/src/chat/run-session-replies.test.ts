import { describe, expect, it, vi } from 'vitest'
import { accepted, base, conversation, pickedModel, required, setup, sourceModel, stored } from './test-support/run-session-fixture'

describe('RunSession replies', () => {
  it('restores graph metadata and sends against the selected parent, not the last candidate', async () => {
    const { session, runs } = setup()
    const source = stored()
    const sibling = stored({ id: 'sibling', supersedesId: source.id })
    session.show('conversation', [source, sibling], source.id)
    expect(session.turns[0]).toMatchObject({
      id: source.id,
      text: source.text,
      questionId: source.questionId,
      ...sourceModel,
      error: source.errorCode,
      persisted: true,
    })
    expect(session.selectedTurnId).toBe(source.id)
    const sent = session.send(base, 'Next question')
    expect(required(runs[0]).request).toMatchObject({ replyToId: source.id, regenerate: false })
    expect(session.turns[2]).toMatchObject({ parentId: source.id, kind: 'reply', ...pickedModel })
    required(runs[0]).accepted.resolve(accepted)
    expect(await sent).toBe(true)
    expect(session.turns[2]).toMatchObject({
      id: 'candidate',
      questionId: 'candidate',
      parentId: source.id,
      persisted: true,
    })
  })


  it('keeps the source and its question when retrying with its original model snapshot', async () => {
    const { session, runs, get } = setup()
    const source = stored({ parentId: 'parent' })
    const candidate = stored({
      id: 'candidate',
      parentId: 'parent',
      supersedesId: source.id,
      text: 'New answer',
    })
    session.show('conversation', [source], source.id)
    get.mockResolvedValue(conversation([source, candidate]))
    const retried = session.retry(base, source.id)
    expect(required(runs[0]).request).toMatchObject({
      retryTurnId: source.id,
      model: sourceModel,
      input: source.input,
      regenerate: false,
    })
    expect(required(runs[0]).request.replyToId).toBeUndefined()
    expect(session.turns[0]).toMatchObject({
      id: source.id,
      text: source.text,
      reasoningText: source.reasoningText,
      questionId: source.questionId,
    })
    expect(session.turns[1]).toMatchObject({
      questionId: source.questionId,
      parentId: source.parentId,
      supersedesId: source.id,
    })
    required(runs[0]).accepted.resolve(accepted)
    await retried
    required(runs[0]).emit({ kind: 'text_delta', text: 'New answer' })
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    await vi.waitFor(() => expect(session.running).toBe(false))
    expect(session.turns).toHaveLength(2)
    expect(required(session.turns[0]).text).toBe(source.text)
    expect(session.selectedTurnId).toBe(source.id)
  })


  it('honors an explicit switch model while keeping the question and source reasoning effort', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored({ reasoningEffort: 'high' })], 'source')
    const retried = session.retry(base, 'source', pickedModel)
    expect(required(runs[0]).request).toMatchObject({
      model: pickedModel,
      reasoningEffort: 'high',
      retryTurnId: 'source',
    })
    required(runs[0]).accepted.resolve(accepted)
    await retried
    expect(session.turns[1]).toMatchObject({
      id: 'candidate',
      questionId: 'question',
      ...pickedModel,
    })
  })


  it('translates the original reply with the real instruction and original model', async () => {
    const { session, runs } = setup()
    const source = stored()
    session.show('conversation', [source], source.id)
    const translated = session.translate({ ...base, model: null }, source.id, ' French ')
    const instruction = `Translate the following reply into French. Return only the translation, preserving Markdown formatting.\n\n${source.text}`
    expect(required(runs[0]).request).toMatchObject({
      translateTurnId: source.id,
      targetLanguage: 'French',
      model: sourceModel,
      input: instruction,
    })
    expect(session.turns[1]).toMatchObject({
      kind: 'translation',
      parentId: source.id,
      supersedesId: null,
      input: instruction,
    })
    expect(required(session.turns[0]).text).toBe(source.text)
    required(runs[0]).accepted.resolve(accepted)
    await translated
    expect(required(session.turns[1]).questionId).toBe('candidate')
  })


  it('honors the translation dialog model selection', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored()], 'source')
    const translated = session.translate(base, 'source', 'French')
    expect(required(runs[0]).request.model).toEqual(pickedModel)
    required(runs[0]).accepted.resolve(accepted)
    await translated
    expect(session.turns[1]).toMatchObject(pickedModel)
  })


  it('preserves translation kind when retrying a translated reply', async () => {
    const { session, runs } = setup()
    session.show('conversation', [stored({ kind: 'translation' })], 'source')
    const retried = session.retry(base, 'source')
    expect(session.turns[1]).toMatchObject({
      kind: 'translation',
      questionId: 'question',
      supersedesId: 'source',
    })
    required(runs[0]).accepted.resolve(accepted)
    await retried
  })


  it('without history appends fresh retry/translation candidates without backend graph fields', async () => {
    const { session, runs } = setup(false)
    const source = stored()
    session.show(null, [source], source.id)
    const retry = session.retry(base, source.id)
    expect(required(runs[0]).request).toMatchObject({
      input: source.input,
      regenerate: false,
      conversationId: null,
      model: sourceModel,
    })
    expect(required(runs[0]).request.retryTurnId).toBeUndefined()
    expect(required(runs[0]).request.replyToId).toBeUndefined()
    required(runs[0]).accepted.resolve({ runId: 'run', conversationId: null, turnId: null })
    await retry
    required(runs[0]).emit({ kind: 'end', status: 'completed' })
    expect(session.turns).toHaveLength(2)
    expect(required(session.turns[0]).text).toBe(source.text)
    expect(session.selectedTurnId).toBe(source.id)
    const translation = session.translate(base, source.id, 'French')
    expect(required(runs[1]).request.translateTurnId).toBeUndefined()
    expect(required(runs[1]).request.targetLanguage).toBeUndefined()
    expect(required(runs[1]).request.input).toContain(source.text)
    required(runs[1]).accepted.resolve({ runId: 'translated', conversationId: null, turnId: null })
    await translation
    expect(session.turns).toHaveLength(3)
  })


})
