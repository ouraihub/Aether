import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import type { ComponentProps } from 'svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  KitClientError,
  type ChatPort,
  type ChatHistoryPort,
  type Conversation,
  type ProvidersPort,
  type ModelOption,
  type RunAccepted,
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
  chat?: Partial<ChatPort>
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
        Promise.resolve([
          {
            ...model,
            label: 'Local · main · m1',
            modelType: 'chat',
            supportsTools: null,
            reasoning: null,
            nativeWebSearch: false,
          },
        ]),
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
      ...extra.chat,
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
  const button = screen.getByRole('button', { name: 'Send' })
  await waitFor(() => expect(button.hasAttribute('disabled')).toBe(false))
  await fireEvent.click(button)
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
    expect(screen.getByText('there')).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Regenerate' })).toHaveLength(1)

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
    expect(screen.queryByRole('button', { name: 'Conversation' })).toBeNull()
    await send('one')
    runs[0]?.emit({ kind: 'end', status: 'failed', error: { code: 'unauthorized', message: 'x' } })
    await screen.findByText('Authentication failed, check the key')
    expect(screen.getAllByRole('button', { name: 'Retry' })).toHaveLength(1)
    expect(screen.queryByRole('button', { name: 'Regenerate' })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
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
    await screen.findByText('Copied')
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
      status: 'completed',
      input: 'earlier',
      text: 'answer',
      modelId: null,
      errorCode: null,
      reasoningEffort: 'default',
      reasoningText: null,
    }
    const summary: Conversation['summary'] = { id: 'c9', title: 'Old', scope: null, createdAt: 0, updatedAt: 0 }
    const get = vi.fn<ChatHistoryPort['get']>(() => Promise.resolve({ summary, turns: [turn] }))
    const onConversationChange = vi.fn()
    const { runs } = setup(true, null, { history: { get }, props: { conversationId: 'c9', onConversationChange } })
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
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'draft' } })
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
    expect(screen.getByRole('link', { name: 'Example' }).getAttribute('href')).toBe('https://example.com')
    expect(screen.getByRole('button', { name: 'Web search' }).getAttribute('aria-pressed')).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: 'Web search' }))
    await send('do not search')
    expect(runs[1]?.request.webSearch).not.toBe(true)
    runs[1]?.emit({ kind: 'end', status: 'completed' })
  })
  it('shows the last request context separately from known cumulative usage and clears it on regeneration', async () => {
    const { runs } = setup(true)
    await send('first')
    const usage = {
      inputTokens: 120,
      outputTokens: 30,
      cacheReadTokens: 80,
      cacheWriteTokens: null,
      reasoningTokens: 20,
    }
    runs[0]?.emit({ kind: 'usage', ...usage })
    runs[0]?.emit({ kind: 'usage', ...usage })
    runs[0]?.emit({ kind: 'context', usage: { tokens: 150, contextWindow: 1000, source: 'reported', ...model } })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    const usageDetails = screen.getByLabelText('Token usage', { selector: 'details' })
    const summary = usageDetails.querySelector('summary')
    if (!summary) throw new Error('Token usage disclosure is missing its summary')
    await fireEvent.click(summary)
    await waitFor(() => expect(screen.getByRole('meter').getAttribute('value')).toBe('15'))
    await send('second')
    runs[1]?.emit({ kind: 'usage', ...usage, inputTokens: 200, outputTokens: 40 })
    runs[1]?.emit({ kind: 'context', usage: { tokens: 240, contextWindow: 1000, source: 'reported', ...model } })
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(screen.getByRole('meter').getAttribute('value')).toBe('24'))
    const inputRow = screen.getByRole('row', { name: 'Input 200 320' })
    expect(inputRow).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: 'Regenerate' }))
    await waitFor(() => expect(screen.queryByRole('meter')).toBeNull())
    expect(screen.queryByRole('meter')).toBeNull()
  })

  it('restores reasoning and statistics from history without inventing unknown fields', async () => {
    setup(true, null, {
      props: { conversationId: 'saved' },
      history: {
        get: () =>
          Promise.resolve({
            summary: { id: 'saved', title: 'Saved', scope: null, createdAt: 0, updatedAt: 0 },
            turns: [
              {
                id: 't',
                status: 'completed',
                input: 'question',
                text: 'answer',
                errorCode: null,
                ...model,
                reasoningEffort: 'high',
                reasoningText: 'summary',
                usage: {
                  inputTokens: 12,
                  outputTokens: 3,
                  cacheReadTokens: null,
                  cacheWriteTokens: null,
                  reasoningTokens: null,
                },
                contextUsage: { tokens: 15, contextWindow: 1000, source: 'reported', ...model },
              },
            ],
          }),
      },
    })
    await screen.findByText('answer')
    const summary = screen.getByLabelText('Token usage', { selector: 'details' }).querySelector('summary')
    if (!summary) throw new Error('Token usage disclosure is missing its summary')
    await fireEvent.click(summary)
    expect(screen.getByRole('meter').getAttribute('value')).toBe('1.5')
    expect(screen.getByText('summary')).toBeTruthy()
    expect(screen.getByRole('row', { name: 'Cache read Unknown Unknown' })).toBeTruthy()
  })

  it('uses Ctrl/Cmd+Enter only, protects IME, and refuses duplicate sends', async () => {
    const { runs } = setup(false)
    const input = screen.getByRole('textbox', { name: 'Message' })
    await fireEvent.input(input, { target: { value: 'first\nsecond' } })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(false))
    await fireEvent.keyDown(input, { key: 'Enter' })
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true, isComposing: true })
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true, keyCode: 229 })
    expect(runs).toHaveLength(0)
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true })
    expect(runs).toHaveLength(1)
    expect(runs[0]?.request.input).toBe('first\nsecond')
    await fireEvent.input(input, { target: { value: 'next draft' } })
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    expect(runs).toHaveLength(1)
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    expect(runs).toHaveLength(2)
    expect(runs[1]?.request.input).toBe('next draft')
  })

  it('uses the same model validation for shortcut and send button', async () => {
    const { runs } = setup(false, null, { props: { model: null } })
    const input = screen.getByRole('textbox', { name: 'Message' })
    await fireEvent.input(input, { target: { value: 'draft' } })
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true })
    expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(true)
    expect(runs).toHaveLength(0)
  })

  it('expands the same textarea without losing draft, selection, or input focus', async () => {
    setup(false)
    const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement
    await fireEvent.input(input, { target: { value: 'a multiline\ndraft' } })
    input.focus()
    input.setSelectionRange(2, 7)
    await fireEvent.mouseDown(screen.getByRole('button', { name: 'Expand input' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Expand input' }))
    expect(screen.getByRole('button', { name: 'Collapse input' }).getAttribute('aria-expanded')).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: 'Collapse input' }))
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(input)
    expect(input.value).toBe('a multiline\ndraft')
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 7])
    expect(document.activeElement).toBe(input)
  })

  it('keeps capability loading, failure, retry and actual no-model states distinct', async () => {
    const loading = Promise.withResolvers<ModelOption[]>()
    const modelOptions = vi
      .fn<ProvidersPort['modelOptions']>()
      .mockReturnValueOnce(loading.promise)
      .mockResolvedValueOnce([])
    const { runs } = setup(false, null, { providers: { modelOptions } })
    expect(screen.queryByText('No chat model yet. Add a provider and its models first.')).toBeNull()
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'draft' } })
    await fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), { key: 'Enter', ctrlKey: true })
    expect(runs).toHaveLength(0)
    loading.reject(new KitClientError({ code: 'network', message: 'offline' }))
    await screen.findByText('Network connection failed')
    expect(screen.queryByText('No chat model yet. Add a provider and its models first.')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Retry loading models' }))
    await screen.findByText('No chat model yet. Add a provider and its models first.')
    expect(modelOptions).toHaveBeenCalledTimes(2)
  })

  it('restores a preaccept-rejected draft and its removed context for the next request', async () => {
    const requests: RunRequest[] = []
    const run = vi.fn<ChatPort['run']>().mockImplementation((request) => {
      requests.push(request)
      return requests.length === 1
        ? Promise.reject(new KitClientError({ code: 'network', message: 'offline' }))
        : Promise.resolve({ runId: 'r2', conversationId: null })
    })
    setup(false, { kind: 'note', id: 'n1', label: 'Draft' }, { chat: { run } })
    await fireEvent.click(screen.getByRole('button', { name: 'Do not send Draft with the next message' }))
    await send('  recover me\nplease  ')
    const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement
    await waitFor(() => expect(input.value).toBe('  recover me\nplease  '))
    expect(screen.queryByText('Draft')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(requests).toHaveLength(2)
    expect(requests[1]?.context).toBeNull()
    expect(requests[1]?.input).toBe('recover me\nplease')
    await screen.findByText('Draft')
  })

  it('does not overwrite a new draft when an earlier request is rejected', async () => {
    const pending = Promise.withResolvers<RunAccepted>()
    setup(false, null, { chat: { run: () => pending.promise } })
    await send('unsent original')
    const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement
    await fireEvent.input(input, { target: { value: 'new draft' } })
    pending.reject(new KitClientError({ code: 'network', message: 'offline' }))
    await screen.findByRole('button', { name: 'Restore unsent message' })
    expect(input.value).toBe('new draft')
    expect(screen.getByRole('button', { name: 'Restore unsent message' }).hasAttribute('disabled')).toBe(true)
    await fireEvent.input(input, { target: { value: '' } })
    await fireEvent.click(screen.getByRole('button', { name: 'Restore unsent message' }))
    expect(input.value).toBe('unsent original')
  })

  it('pauses streaming scroll while reading upward and keeps input focus', async () => {
    const { runs } = setup(false)
    await send('long answer')
    const messages = screen.getByRole('region', { name: 'Messages' })
    Object.defineProperties(messages, {
      scrollHeight: { value: 1000, configurable: true },
      clientHeight: { value: 200, configurable: true },
    })
    messages.scrollTop = 800
    await fireEvent.scroll(messages)
    messages.scrollTop = 300
    await fireEvent.scroll(messages)
    const input = screen.getByRole('textbox', { name: 'Message' })
    input.focus()
    runs[0]?.emit({ kind: 'text_delta', text: 'first output' })
    await screen.findByText('first output')
    expect(messages.scrollTop).toBe(300)
    expect(document.activeElement).toBe(input)
    await fireEvent.click(screen.getByRole('button', { name: 'Back to bottom' }))
    expect(messages.scrollTop).toBe(1000)
    Object.defineProperty(messages, 'scrollHeight', { value: 1200, configurable: true })
    runs[0]?.emit({ kind: 'text_delta', text: ' continued' })
    await waitFor(() => expect(messages.scrollTop).toBe(1200))
  })

  it('renames through history and retains current messages when deletion fails', async () => {
    let saved = 'Saved'
    const remove = vi.fn<ChatHistoryPort['remove']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    const summary = () => ({ id: 'c9', title: saved, scope: null, createdAt: 0, updatedAt: 0 })
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
                input: 'earlier',
                text: 'answer',
                status: 'completed',
                modelId: null,
                errorCode: null,
                reasoningEffort: 'default',
                reasoningText: null,
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
    await fireEvent.input(screen.getByRole('textbox', { name: 'Conversation title' }), { target: { value: 'Renamed' } })
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

  it('offers only one last-turn action and reports clipboard and insertion failures', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: () => Promise.reject(new Error('denied')) },
      configurable: true,
    })
    const { runs, notify } = setup(false, null, {
      props: {
        onInsert: () => {
          throw new Error('read only')
        },
      },
    })
    await send('one')
    runs[0]?.emit({ kind: 'text_delta', text: 'answer one' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await send('two')
    runs[1]?.emit({ kind: 'text_delta', text: 'answer two' })
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('answer two')
    expect(screen.getAllByRole('button', { name: 'Regenerate' })).toHaveLength(1)
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull()
    const copy = screen.getAllByRole('button', { name: 'Copy' })[1]
    if (!copy) throw new Error('Second reply is missing its Copy action')
    await fireEvent.click(copy)
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Could not copy'))
    const insert = screen.getAllByRole('button', { name: 'Insert' })[1]
    if (!insert) throw new Error('Second reply is missing its Insert action')
    await fireEvent.click(insert)
    expect(notify.toast).toHaveBeenCalledWith('error', 'Internal error')
    expect(screen.getByRole('region', { name: 'Messages' }).hasAttribute('aria-live')).toBe(false)
  })

  it('exposes only configured reasoning choices and sends default without changing its meaning', async () => {
    const { runs } = setup(false, null, {
      providers: {
        modelOptions: () =>
          Promise.resolve([
            {
              ...model,
              label: 'Reasoner',
              modelType: 'chat',
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['high'] },
              nativeWebSearch: false,
            },
          ]),
      },
    })
    const effort = await screen.findByLabelText('Reasoning effort')
    await fireEvent.keyDown(effort, { key: 'ArrowDown' })
    expect(await screen.findByRole('option', { name: 'High' })).toBeTruthy()
    expect(screen.queryByRole('option', { name: 'Medium' })).toBeNull()
    await fireEvent.click(screen.getByRole('option', { name: 'Default' }))
    await send('default request')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.keyDown(screen.getByLabelText('Reasoning effort'), { key: 'ArrowDown' })
    await fireEvent.keyDown(screen.getByLabelText('Reasoning effort'), { key: 'End' })
    await fireEvent.keyDown(screen.getByLabelText('Reasoning effort'), { key: 'Enter' })
    await send('explicit request')
    expect(runs[1]?.request.reasoningEffort).toBe('high')
  })

  it('retries a failed history restore without losing the saved conversation or replacing its identity', async () => {
    const onConversationChange = vi.fn()
    const get = vi
      .fn<ChatHistoryPort['get']>()
      .mockRejectedValueOnce(new KitClientError({ code: 'network', message: 'offline' }))
      .mockResolvedValueOnce({
        summary: { id: 'saved', title: 'Saved', scope: null, createdAt: 0, updatedAt: 0 },
        turns: [
          {
            id: 't',
            input: 'saved question',
            text: 'saved answer',
            status: 'completed',
            modelId: null,
            errorCode: null,
            reasoningEffort: 'default',
            reasoningText: null,
          },
        ],
      })
    setup(true, null, { props: { conversationId: 'saved', onConversationChange }, history: { get } })
    await screen.findByText('Network connection failed')
    await fireEvent.click(screen.getByRole('button', { name: 'Retry conversation operation' }))
    await screen.findByText('saved answer')
    expect(screen.queryByText('Network connection failed')).toBeNull()
    expect(onConversationChange).not.toHaveBeenCalled()
    expect(get).toHaveBeenCalledTimes(2)
  })

  it('hides insert without a host capability and hides reasoning for incapable models', async () => {
    const { runs } = setup(false)
    await send('hi')
    runs[0]?.emit({ kind: 'text_delta', text: 'reply' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('reply')
    expect(screen.queryByRole('button', { name: 'Insert' })).toBeNull()
    expect(screen.queryByRole('combobox', { name: 'Reasoning effort' })).toBeNull()
  })

  it('renders streamed Markdown structure while stripping executable HTML and dangerous links', async () => {
    const { runs } = setup(false)
    await send('markdown')
    runs[0]?.emit({
      kind: 'text_delta',
      text: '## Heading\n\n- first\n- second\n\n```ts\nconst safe = true\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n<script>window.hacked = true</script><img src="x" onerror="window.hacked=true"><a href="javascript:alert(1)">unsafe</a>',
    })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByRole('heading', { name: 'Heading' })
    const messages = screen.getByRole('region', { name: 'Messages' })
    expect(messages.querySelectorAll('ul > li')).toHaveLength(2)
    expect(messages.querySelector('pre code')?.textContent).toContain('const safe = true')
    expect(messages.querySelectorAll('table tbody tr')).toHaveLength(1)
    expect(messages.querySelector('script, [onerror], a[href^="javascript:"]')).toBeNull()
  })
})
