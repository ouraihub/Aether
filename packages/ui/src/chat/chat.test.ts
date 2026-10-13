import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import type { ComponentProps } from 'svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  type ChatHistoryPort,
  type ProvidersPort,
  type RunEvent,
  type RunRequest,
  type SearchPort,
} from '../ports'
import type ChatPanel from './ChatPanel.svelte'
import Harness from './Harness.test.svelte'

const model = { endpointId: 'e1', modelId: 'm1' }

interface Run {
  request: RunRequest
  emit: (event: RunEvent) => void
  signal: AbortSignal | undefined
}

interface Extra {
  props?: Partial<ComponentProps<typeof ChatPanel>>
  history?: Partial<ChatHistoryPort>
  providers?: Partial<ProvidersPort>
  search?: Partial<SearchPort>
}

function setup(
  withHistory: boolean,
  context: { kind: string; id: string; label: string } | null = null,
  extra: Extra = {},
) {
  const runs: Run[] = []
  const approve = vi.fn<(runId: string, callId: string, allow: boolean) => Promise<void>>(() => Promise.resolve())
  const ports = createFakePorts({
    providers: {
      modelOptions: () =>
        Promise.resolve([{ ...model, label: 'Local · main · m1', modelType: 'chat', supportsTools: null }]),
      ...extra.providers,
    },
    ...(extra.search ? { search: extra.search } : {}),
    chat: {
      run: (request, emit, options) => {
        runs.push({ request, emit, signal: options?.signal })
        options?.signal?.addEventListener('abort', () => emit({ kind: 'end', status: 'cancelled' }))
        return Promise.resolve({ runId: `r${runs.length}`, conversationId: withHistory ? 'c1' : null })
      },
      approve,
    },
  })
  if (ports.chat?.history) Object.assign(ports.chat.history, extra.history)
  if (!withHistory && ports.chat) delete ports.chat.history
  const notify = { toast: vi.fn(), confirm: vi.fn(() => Promise.resolve(true)) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const kit = { ports, i18n: createI18n('en'), notify, logger }
  const view = render(Harness, { props: { kit, model, context, ...extra.props } })
  return { runs, view, approve, notify }
}

async function send(text: string) {
  await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: text } })
  await fireEvent.click(screen.getByRole('button', { name: 'Send' }))
}

describe('ChatPanel', () => {
  it('streams, stops as cancelled, then regenerates the stored turn', async () => {
    const { runs } = setup(true)
    await send('hello')
    const first = runs[0]
    expect(first?.request).toMatchObject({ input: 'hello', model, regenerate: false, conversationId: null })
    first?.emit({ kind: 'text_delta', text: 'Hi **there**' })
    await screen.findByText('there')
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    expect(first?.signal?.aborted).toBe(true)
    await screen.findByText('Stopped')
    expect(screen.getAllByText('Stopped')).toHaveLength(1)

    await fireEvent.click(screen.getByRole('button', { name: 'Regenerate' }))
    const second = runs[1]
    expect(second?.request).toMatchObject({ regenerate: true, conversationId: 'c1' })
    second?.emit({ kind: 'text_delta', text: 'Again' })
    second?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('Again')
    await waitFor(() => expect(screen.queryByText('Stopped')).toBeNull())
    expect(screen.getAllByRole('listitem')).toHaveLength(1)
  })

  it('without history sends fresh runs and hides the conversation list', async () => {
    const { runs } = setup(false)
    expect(screen.queryByRole('combobox', { name: 'Conversation' })).toBeNull()
    await send('one')
    runs[0]?.emit({ kind: 'end', status: 'failed', error: { code: 'unauthorized', message: 'x' } })
    await screen.findByText('Authentication failed, check the key')
    await fireEvent.click(screen.getByRole('button', { name: 'Regenerate' }))
    expect(runs[1]?.request).toMatchObject({ input: 'one', regenerate: false, conversationId: null })
  })

  it('sends the host context, drops it for one message when removed, then sends it again', async () => {
    const { runs } = setup(false, { kind: 'note', id: 'n1', label: 'Draft' })
    await send('one')
    expect(runs[0]?.request.context).toEqual({ kind: 'note', id: 'n1' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Do not send Draft with the next message' }))
    expect(screen.queryByText('Draft')).toBeNull()
    await send('two')
    expect(runs[1]?.request.context).toBeNull()
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('Draft')
    await send('three')
    expect(runs[2]?.request.context).toEqual({ kind: 'note', id: 'n1' })
  })

  it('unmounting aborts the running run', async () => {
    const { runs, view } = setup(true)
    await send('hello')
    view.unmount()
    expect(runs[0]?.signal?.aborted).toBe(true)
  })

  it('shows tool calls, asks for approval, and drops the card on stop', async () => {
    const { runs, approve } = setup(true)
    await send('edit')
    const run = runs[0]
    run?.emit({ kind: 'tool_call', callId: 'c1', name: 'read_note', source: 'host' })
    run?.emit({ kind: 'tool_result', callId: 'c1', outcome: 'ok', summary: 'two lines' })
    await screen.findByText('read_note: two lines')

    run?.emit({ kind: 'tool_call', callId: 'c2', name: 'write_note', source: 'host' })
    run?.emit({ kind: 'approval_required', callId: 'c2', name: 'write_note', argsPreview: '{"text":"x"}' })
    await screen.findByText('{"text":"x"}')
    await fireEvent.click(screen.getByRole('button', { name: 'Allow once' }))
    expect(approve).toHaveBeenLastCalledWith('r1', 'c2', true)
    expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull()
    await screen.findByText('Calling write_note')

    run?.emit({ kind: 'tool_call', callId: 'c3', name: 'write_note', source: 'host' })
    run?.emit({ kind: 'approval_required', callId: 'c3', name: 'write_note', argsPreview: '{}' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Deny' }))
    expect(approve).toHaveBeenLastCalledWith('r1', 'c3', false)
    run?.emit({ kind: 'tool_result', callId: 'c3', outcome: 'denied', summary: 'denied' })
    await screen.findByText('write_note: denied')

    run?.emit({ kind: 'tool_call', callId: 'c4', name: 'write_note', source: 'host' })
    run?.emit({ kind: 'approval_required', callId: 'c4', name: 'write_note', argsPreview: '{}' })
    await screen.findByRole('button', { name: 'Allow once' })
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await screen.findByText('Stopped')
    expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull()
    expect(approve).toHaveBeenCalledTimes(2)
  })

  it('inserts the finished turn markdown', async () => {
    const onInsert = vi.fn()
    const { runs } = setup(false, null, { props: { onInsert } })
    await send('hi')
    runs[0]?.emit({ kind: 'text_delta', text: 'Hi **there**' })
    expect((await screen.findByRole('button', { name: 'Insert' })).hasAttribute('disabled')).toBe(true)
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Insert' }).hasAttribute('disabled')).toBe(false))
    await fireEvent.click(screen.getByRole('button', { name: 'Insert' }))
    expect(onInsert).toHaveBeenCalledWith('Hi **there**')
  })

  it('copies the turn markdown to the clipboard', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    const { runs } = setup(false)
    await send('hi')
    runs[0]?.emit({ kind: 'text_delta', text: '`code`' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy' }))
    expect(writeText).toHaveBeenCalledWith('`code`')
    await screen.findByRole('button', { name: 'Copied' })
  })

  it('retries a failed turn by regenerating it', async () => {
    const { runs } = setup(true)
    await send('hi')
    runs[0]?.emit({ kind: 'end', status: 'failed', error: { code: 'upstream', message: 'x' } })
    await fireEvent.click(await screen.findByRole('button', { name: 'Retry' }))
    expect(runs[1]?.request).toMatchObject({ regenerate: true, conversationId: 'c1' })
  })

  it('deletes the current conversation after confirming, then starts a new one', async () => {
    const remove = vi.fn(() => Promise.resolve())
    const onConversationChange = vi.fn()
    const { runs, notify } = setup(true, null, { history: { remove }, props: { onConversationChange } })
    await send('hi')
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Delete conversation' }))
    expect(notify.confirm).toHaveBeenCalledOnce()
    await waitFor(() => expect(remove).toHaveBeenCalledWith('c1'))
    await screen.findByText('Ask anything to start')
    expect(screen.queryByRole('listitem')).toBeNull()
    expect(onConversationChange).toHaveBeenLastCalledWith(null)
  })

  it('restores the given conversation and continues it', async () => {
    const turn = {
      id: 't1',
      status: 'completed' as const,
      input: 'earlier',
      text: 'answer',
      modelId: null,
      errorCode: null,
    }
    const summary = { id: 'c9', title: 'Old', scope: null, createdAt: 0, updatedAt: 0 }
    const get = vi.fn(() => Promise.resolve({ summary, turns: [turn] }))
    const onConversationChange = vi.fn()
    const { runs } = setup(true, null, { history: { get }, props: { conversationId: 'c9', onConversationChange } })
    await screen.findByText('earlier')
    expect(get).toHaveBeenCalledWith('c9')
    expect(onConversationChange).not.toHaveBeenCalled()
    await send('more')
    expect(runs[0]?.request.conversationId).toBe('c9')
  })

  it('starts a new conversation when the given one is gone', async () => {
    const onConversationChange = vi.fn()
    const { notify } = setup(true, null, { props: { conversationId: 'gone', onConversationChange } })
    await waitFor(() => expect(onConversationChange).toHaveBeenCalledWith(null))
    expect(notify.toast).not.toHaveBeenCalled()
  })

  it('reports the conversation a run creates', async () => {
    const onConversationChange = vi.fn()
    setup(true, null, { props: { onConversationChange } })
    await send('hi')
    await waitFor(() => expect(onConversationChange).toHaveBeenCalledWith('c1'))
  })

  it('without chat models offers to open settings', async () => {
    const onOpenSettings = vi.fn()
    setup(false, null, { providers: { modelOptions: () => Promise.resolve([]) }, props: { onOpenSettings } })
    await fireEvent.click(await screen.findByRole('button', { name: 'Open settings' }))
    expect(onOpenSettings).toHaveBeenCalledOnce()
  })

  it('Esc closes the panel unless a run is active', async () => {
    const onClose = vi.fn()
    setup(false, null, { props: { onClose } })
    const input = screen.getByRole('textbox', { name: 'Message' })
    await send('hi')
    await fireEvent.keyDown(input, { key: 'Escape' })
    expect(onClose).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await fireEvent.keyDown(input, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledOnce()
  })
  it('sends explicit web search and keeps sources on its turn', async () => {
    const { runs } = setup(false, null, { search: {} })
    await fireEvent.click(screen.getByRole('button', { name: 'Web search' }))
    await send('latest news')
    expect(runs[0]?.request).toMatchObject({ webSearch: true, input: 'latest news' })
    runs[0]?.emit({ kind: 'search_start' })
    runs[0]?.emit({
      kind: 'search_result',
      query: 'latest news',
      provider: 'jina',
      hits: [{ title: 'Example', url: 'https://example.com', snippet: 'Result' }],
    })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByRole('link', { name: 'Example' })
    expect(screen.getByText('Found 1 results')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Web search' }).getAttribute('aria-pressed')).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: 'Web search' }))
    await send('do not search')
    expect(runs[1]?.request.webSearch).not.toBe(true)
    runs[1]?.emit({ kind: 'end', status: 'completed' })
  })
})
