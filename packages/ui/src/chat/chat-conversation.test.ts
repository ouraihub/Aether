import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError } from '../ports'
import type { ConversationClient, Conversation } from '../ports'
import { setup, send } from './test-support/chat-fixture'

describe('ChatPanel conversations', () => {
  it('deletes the current conversation after confirming, then starts a new one', async () => {
    const remove = vi.fn(() => Promise.resolve())
    const onConversationChange = vi.fn()
    const { runs, notify } = setup(true, null, {
      history: { remove },
      props: { onConversationChange },
    })
    await send('hi')
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(screen.getByRole('button', { name: 'Conversation' }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Delete conversation' }))
    expect(notify.confirm).toHaveBeenCalledOnce()
    await waitFor(() => expect(remove).toHaveBeenCalledWith('c1'))
    await screen.findByText('Ask anything to start')
    expect(screen.queryByRole('listitem')).toBeNull()
    expect(onConversationChange).toHaveBeenLastCalledWith(null)
  })

  it('restores the given conversation and continues it', async () => {
    const turn: Conversation['turns'][number] = {
      id: 't1',
      parentId: null,
      questionId: 't1',
      supersedesId: null,
      kind: 'reply',
      endpointId: null,
      status: 'completed',
      input: 'earlier',
      text: 'answer',
      modelId: null,
      errorCode: null,
      reasoningEffort: 'default',
      reasoningText: null,
      images: [],
      generated: [],
    }
    const summary: Conversation['summary'] = {
      id: 'c9',
      title: 'Old',
      scope: null,
      selectedTurnId: 't1',
      createdAt: 0,
      updatedAt: 0,
    }
    const get = vi.fn<ConversationClient['get']>(() => Promise.resolve({ summary, turns: [turn] }))
    const onConversationChange = vi.fn()
    const { runs } = setup(true, null, {
      history: { get },
      props: { conversationId: 'c9', onConversationChange },
    })
    await screen.findByText('earlier')
    expect(get).toHaveBeenCalledWith('c9')
    expect(onConversationChange).not.toHaveBeenCalled()
    await send('more')
    expect(runs[0]?.request.conversationId).toBe('c9')
  })

  it('keeps a failed restore visible and does not silently replace the saved conversation', async () => {
    const onConversationChange = vi.fn()
    setup(true, null, { props: { conversationId: 'gone', onConversationChange } })
    await screen.findByText('Not found')
    expect(onConversationChange).not.toHaveBeenCalled()
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'draft' },
    })
    expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(true)
    await fireEvent.click(screen.getByRole('button', { name: 'New conversation' }))
    await waitFor(() => expect(onConversationChange).toHaveBeenCalledWith(null))
  })

  it('reports the conversation a run creates', async () => {
    const onConversationChange = vi.fn()
    setup(true, null, { props: { onConversationChange } })
    await send('hi')
    await waitFor(() => expect(onConversationChange).toHaveBeenCalledWith('c1'))
  })

  it('renames through history and retains current messages when deletion fails', async () => {
    let saved = 'Saved'
    const remove = vi.fn<ConversationClient['remove']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    const summary = () => ({
      id: 'c9',
      title: saved,
      scope: null,
      selectedTurnId: 't',
      createdAt: 0,
      updatedAt: 0,
    })
    setup(true, null, {
      props: { conversationId: 'c9' },
      history: {
        list: () => Promise.resolve([summary()]),
        get: () =>
          Promise.resolve({
            summary: summary(),
            turns: [
              {
                id: 't',
                parentId: null,
                questionId: 't',
                supersedesId: null,
                kind: 'reply',
                endpointId: null,
                input: 'earlier',
                text: 'answer',
                status: 'completed',
                modelId: null,
                errorCode: null,
                reasoningEffort: 'default',
                reasoningText: null,
                images: [],
                generated: [],
              },
            ],
          }),
        rename: (_id, title) => {
          saved = title
          return Promise.resolve(summary())
        },
        remove,
      },
    })
    await screen.findByText('answer')
    await fireEvent.click(screen.getByRole('button', { name: 'Conversation' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Rename conversation' }))
    await fireEvent.input(screen.getByRole('textbox', { name: 'Conversation title' }), {
      target: { value: 'Renamed' },
    })
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    await waitFor(() => expect(saved).toBe('Renamed'))
    await fireEvent.click(screen.getByRole('button', { name: 'Delete conversation' }))
    await screen.findByText(/Network connection failed/)
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.getByRole('heading', { name: 'Renamed' })).toBeTruthy()
    expect(screen.getByText('answer')).toBeTruthy()
    expect(remove).toHaveBeenCalledWith('c9')
  })

  it('closes history with Escape without closing the panel', async () => {
    const onClose = vi.fn()
    setup(true, null, { props: { onClose } })
    await fireEvent.click(screen.getByRole('button', { name: 'Conversation' }))
    const dialog = await screen.findByRole('dialog')
    await fireEvent.keyDown(dialog, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(onClose).not.toHaveBeenCalled()
  })

  it('retries a failed history restore without losing the saved conversation or replacing its identity', async () => {
    const onConversationChange = vi.fn()
    const get = vi
      .fn<ConversationClient['get']>()
      .mockRejectedValueOnce(new KitClientError({ code: 'network', message: 'offline' }))
      .mockResolvedValueOnce({
        summary: {
          id: 'saved',
          title: 'Saved',
          scope: null,
          selectedTurnId: 't',
          createdAt: 0,
          updatedAt: 0,
        },
        turns: [
          {
            id: 't',
            parentId: null,
            questionId: 't',
            supersedesId: null,
            kind: 'reply',
            endpointId: null,
            input: 'saved question',
            text: 'saved answer',
            status: 'completed',
            modelId: null,
            errorCode: null,
            reasoningEffort: 'default',
            reasoningText: null,
            images: [],
            generated: [],
          },
        ],
      })
    setup(true, null, {
      props: { conversationId: 'saved', onConversationChange },
      history: { get },
    })
    await screen.findByText('Network connection failed')
    await fireEvent.click(screen.getByRole('button', { name: 'Retry conversation operation' }))
    await screen.findByText('saved answer')
    expect(screen.queryByText('Network connection failed')).toBeNull()
    expect(onConversationChange).not.toHaveBeenCalled()
    expect(get).toHaveBeenCalledTimes(2)
  })

})
