import { fireEvent, screen, waitFor, within } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError } from '../ports'
import type { ConversationClient, RunClient } from '../ports'
import { model, required, setup, send } from './test-support/chat-fixture'
import { candidates } from './test-support/conversation-fixtures'

describe('ChatPanel conversation actions', () => {
  it('renders full same-question candidates once and sets context via 👍 without forking', async () => {
    const conversation = candidates()
    const selectTurn = vi.fn<ConversationClient['selectTurn']>((_id, turnId) =>
      Promise.resolve({
        ...conversation,
        summary: { ...conversation.summary, selectedTurnId: turnId },
      }),
    )
    const fork = vi.fn<ConversationClient['fork']>(() => Promise.resolve(conversation))
    const { runs } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), selectTurn, fork },
    })
    await screen.findByText('Alternative reply')
    expect(screen.getAllByText('Shared question')).toHaveLength(1)
    expect(screen.getByText('reply', { selector: 'strong' })).toBeTruthy()
    expect(screen.getAllByText('m1', { selector: '.reply-source' })).toHaveLength(2)
    const setContext = screen.getAllByRole('button', { name: 'Set as context' })
    expect(setContext.map((button) => button.getAttribute('aria-pressed'))).toEqual([
      'true',
      'false',
    ])
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Keep my draft' },
    })
    await fireEvent.click(required(setContext[1]))
    await waitFor(() => expect(selectTurn).toHaveBeenCalledWith('saved', 'b'))
    expect(fork).not.toHaveBeenCalled()
    expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe(
      'Keep my draft',
    )
    await send('Continue chosen reply')
    expect(runs[0]?.request).toMatchObject({ conversationId: 'saved', replyToId: 'b' })
  })

  it('edits with compare-and-swap and retains both saved reply and local draft on conflict', async () => {
    const conversation = candidates()
    const editTurn = vi
      .fn<ConversationClient['editTurn']>()
      .mockRejectedValueOnce(new KitClientError({ code: 'conflict', message: 'changed' }))
      .mockImplementationOnce((_id, turnId, text) =>
        Promise.resolve({
          ...conversation,
          turns: conversation.turns.map((turn) => (turn.id === turnId ? { ...turn, text } : turn)),
        }),
      )
    setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), editTurn },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Edit' }))
    const editor = (await screen.findByRole('textbox', {
      name: 'Reply Markdown',
    })) as HTMLTextAreaElement
    await fireEvent.input(editor, { target: { value: 'Edited local draft' } })
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    await screen.findByText('Conflicts with existing data')
    expect(editor.value).toBe('Edited local draft')
    expect(screen.getByText('Alternative reply')).toBeTruthy()
    expect(editTurn).toHaveBeenLastCalledWith(
      'saved',
      'b',
      'Edited local draft',
      'Alternative reply',
    )
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    await screen.findByText('Edited local draft')
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('regenerates any source snapshot and switches model to another same-conversation candidate', async () => {
    const conversation = candidates()
    const secondModel = { endpointId: 'e2', modelId: 'm2' }
    const { runs } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
      providers: {
        modelOptions: () =>
          Promise.resolve([
            {
              ...model,
              label: 'First model',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: null,
              nativeWebSearch: false,
              canVision: false,
            },
            {
              ...secondModel,
              label: 'Second model',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: null,
              nativeWebSearch: false,
              canVision: false,
            },
          ]),
      },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Regenerate' })[0]))
    expect(runs[0]?.request).toMatchObject({
      conversationId: 'saved',
      retryTurnId: 'a',
      model,
      regenerate: false,
    })
    expect(screen.getByText('reply', { selector: 'strong' })).toBeTruthy()
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() =>
      expect(
        screen.getAllByRole('button', { name: 'Switch model' })[1]?.hasAttribute('disabled'),
      ).toBe(false),
    )
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Switch model' })[1]))
    expect(screen.queryByRole('dialog')).toBeNull()
    await fireEvent.click(await screen.findByRole('option', { name: /m2/ }))
    expect(runs[1]?.request).toMatchObject({
      conversationId: 'saved',
      retryTurnId: 'b',
      model: secondModel,
    })
  })

  it('translates actual source text with source ID and chosen language while retaining the source', async () => {
    const conversation = candidates()
    const { runs } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Translate' })[1]))
    await fireEvent.input(await screen.findByRole('textbox', { name: 'Target language' }), {
      target: { value: 'French' },
    })
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    expect(runs[0]?.request).toMatchObject({
      conversationId: 'saved',
      translateTurnId: 'b',
      targetLanguage: 'French',
      model,
      regenerate: false,
    })
    expect(runs[0]?.request.input).toContain('Alternative reply')
    expect(runs[0]?.request.input).toContain('French')
    expect(screen.getByText('Alternative reply')).toBeTruthy()
  })

  it('hides set-as-context for an only reply and deletes it after confirmation', async () => {
    const conversation = candidates()
    conversation.turns = [required(conversation.turns[0])]
    const removeTurn = vi.fn<ConversationClient['removeTurn']>(() =>
      Promise.resolve({
        ...conversation,
        summary: { ...conversation.summary, selectedTurnId: null },
        turns: [],
      }),
    )
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), removeTurn },
    })
    await screen.findByText('reply', { selector: 'strong' })
    expect(screen.queryByRole('button', { name: 'Set as context' })).toBeNull()
    const remove = screen.getByRole('button', { name: 'Delete' })
    expect(remove.hasAttribute('disabled')).toBe(false)
    notify.confirm.mockResolvedValueOnce(false)
    await fireEvent.click(remove)
    await waitFor(() => expect(notify.confirm).toHaveBeenCalledOnce())
    expect(removeTurn).not.toHaveBeenCalled()
    await fireEvent.click(remove)
    await waitFor(() => expect(removeTurn).toHaveBeenCalledWith('saved', 'a'))
    expect(notify.confirm).toHaveBeenLastCalledWith({
      title: 'Delete this message?',
      body: '',
      danger: true,
    })
    await waitFor(() => expect(screen.queryByText('Shared question')).toBeNull())
  })

  it('branches and copies via the selected ancestor chain and only shows save-note for a host callback', async () => {
    const conversation = candidates()
    const fork = vi.fn<ConversationClient['fork']>(() =>
      Promise.resolve({ ...conversation, summary: { ...conversation.summary, id: 'forked' } }),
    )
    const selectTurn = vi.fn<ConversationClient['selectTurn']>((_id, turnId) =>
      Promise.resolve({
        ...conversation,
        summary: { ...conversation.summary, selectedTurnId: turnId },
      }),
    )
    const onSaveNote = vi.fn(() => Promise.reject(new Error('disk full')))
    const onConversationChange = vi.fn()
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved', onSaveNote, onConversationChange },
      history: { get: () => Promise.resolve(conversation), fork, selectTurn },
    })
    const card = required((await screen.findByText('Alternative reply')).closest('li'))
    const saveNote = within(card).getByRole('button', { name: 'Save to note' })
    await fireEvent.click(saveNote)
    await waitFor(() =>
      expect(notify.toast).toHaveBeenCalledWith('error', 'Could not save the note'),
    )
    expect(onSaveNote).toHaveBeenCalledWith('Alternative reply', [])
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Discard for branch' },
    })
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Branch' }))
    await waitFor(() => expect(selectTurn).toHaveBeenCalledWith('saved', 'b'))
    expect(fork).not.toHaveBeenCalled()
    expect(onConversationChange).not.toHaveBeenCalled()
    expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('')
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[0]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy as new conversation' }))
    await waitFor(() => expect(fork).toHaveBeenLastCalledWith('saved', 'a'))
    await waitFor(() => expect(onConversationChange).toHaveBeenCalledWith('forked'))
  })



  it('keeps deletion of a reply ancestor enabled with a reparent warning but disables editing it', async () => {
    const conversation = candidates()
    conversation.summary.selectedTurnId = 'b'
    required(conversation.turns[1]).parentId = 'a'
    const removeTurn = vi.fn<ConversationClient['removeTurn']>(() =>
      Promise.resolve({
        ...conversation,
        turns: [{ ...required(conversation.turns[1]), parentId: null }],
      }),
    )
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), removeTurn },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[0]))
    const edit = await screen.findByRole('menuitem', { name: 'Edit' })
    expect(edit.getAttribute('aria-disabled')).toBe('true')
    await fireEvent.keyDown(edit, { key: 'Escape' })
    const deletion = required(screen.getAllByRole('button', { name: 'Delete' })[0])
    expect(deletion.hasAttribute('disabled')).toBe(false)
    await fireEvent.click(deletion)
    await waitFor(() => expect(removeTurn).toHaveBeenCalledWith('saved', 'a'))
    expect(notify.confirm).toHaveBeenCalledWith(
      expect.objectContaining({ body: expect.stringContaining('Later turns will attach') }),
    )
  })

  it('keeps reply dialogs keyboard-dismissable without closing the host and disables mutations while running', async () => {
    const conversation = candidates()
    const onClose = vi.fn()
    const { runs } = setup(true, null, {
      props: { conversationId: 'saved', onClose },
      history: { get: () => Promise.resolve(conversation) },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Edit' }))
    const editor = await screen.findByRole('textbox', { name: 'Reply Markdown' })
    await fireEvent.keyDown(editor, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(onClose).not.toHaveBeenCalled()
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Regenerate' })[0]))
    expect(runs[0]?.request.retryTurnId).toBe('a')
    for (const label of ['Switch model', 'Translate', 'Delete', 'Set as context']) {
      expect(
        screen
          .getAllByRole('button', { name: label })
          .every((button) => button.hasAttribute('disabled')),
      ).toBe(true)
    }
  })

  it('retains translation language and original candidates when acceptance fails', async () => {
    const conversation = candidates()
    const run = vi.fn<RunClient['start']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
      chat: { start: run },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Translate' })[1]))
    const language = (await screen.findByRole('textbox', {
      name: 'Target language',
    })) as HTMLInputElement
    await fireEvent.input(language, { target: { value: '日本語' } })
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    await waitFor(() =>
      expect(screen.getByRole('dialog').textContent).toContain('Network connection failed'),
    )
    expect(language.value).toBe('日本語')
    expect(screen.getByText('Alternative reply')).toBeTruthy()
    expect(screen.getByText('reply', { selector: 'strong' })).toBeTruthy()
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
  })

  it('keeps branch drafts and selection unchanged when context persistence rejects', async () => {
    const conversation = candidates()
    const selectTurn = vi.fn<ConversationClient['selectTurn']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), selectTurn },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Keep on failure' },
    })
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Branch' }))
    await waitFor(() =>
      expect(notify.toast).toHaveBeenCalledWith('error', 'Network connection failed'),
    )
    expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe(
      'Keep on failure',
    )
    expect(
      screen.getAllByRole('button', { name: 'Set as context' })[0]?.getAttribute('aria-pressed'),
    ).toBe('true')
  })
  // Candidate and reply-action tests are migrated here from chat-conversation.test.ts.
})
