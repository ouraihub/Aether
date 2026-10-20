import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import type { ComponentProps } from 'svelte'
import { beforeEach, describe, expect, it, vi } from 'vitest'
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
import { providersRevision } from '../providers/revision.svelte'

const model = { endpointId: 'e1', modelId: 'm1' }

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected chat fixture or control is missing')
  return value
}

interface Run {
  request: RunRequest
  turnId: string
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
  const stored: Conversation = {
    summary: { id: 'c1', title: 'Test', scope: null, selectedTurnId: null, createdAt: 0, updatedAt: 0 },
    turns: [],
  }
  const approve = vi.fn<(runId: string, callId: string, allow: boolean) => Promise<void>>(() => Promise.resolve())
  const ports = createFakePorts({
    providers: {
      modelOptions: () =>
        Promise.resolve([
          {
            ...model,
            label: 'Local · main · m1',
            modelType: 'chat',
            name: null,
            supportsTools: null,
            reasoning: null,
            nativeWebSearch: false,
            canVision: false,
          },
        ]),
      ...extra.providers,
    },
    ...(extra.search ? { search: extra.search } : {}),
    chat: {
      run: (request, emit, options) => {
        const id = crypto.randomUUID()
        const source = stored.turns.find((turn) => turn.id === (request.retryTurnId ?? request.translateTurnId))
        const turn: Conversation['turns'][number] = {
          id,
          parentId: request.retryTurnId
            ? (source?.parentId ?? null)
            : (request.translateTurnId ?? request.replyToId ?? stored.summary.selectedTurnId),
          questionId: request.retryTurnId ? (source?.questionId ?? id) : id,
          supersedesId: request.retryTurnId ?? null,
          kind: request.translateTurnId ? 'translation' : 'reply',
          input: source?.input ?? request.input,
          images: source
            ? source.images
            : (request.images ?? []).map((image) => ({
                mime: image.mime,
                url: `data:${image.mime};base64,${image.data}`,
              })),
          generated: [],
          text: '',
          status: 'running',
          errorCode: null,
          endpointId: request.model?.endpointId ?? null,
          modelId: request.model?.modelId ?? null,
          reasoningEffort: request.reasoningEffort ?? 'default',
          reasoningText: null,
        }
        stored.turns.push(turn)
        const wrapped = (event: RunEvent) => {
          if (event.kind === 'text_delta') turn.text += event.text
          if (event.kind === 'reasoning_delta') turn.reasoningText = (turn.reasoningText ?? '') + event.text
          if (event.kind === 'usage') {
            turn.usage = {
              inputTokens: event.inputTokens,
              outputTokens: event.outputTokens,
              cacheReadTokens: event.cacheReadTokens,
              cacheWriteTokens: event.cacheWriteTokens,
              reasoningTokens: event.reasoningTokens,
            }
            turn.cost = event.cost
          }
          if (event.kind === 'context') turn.contextUsage = event.usage
          if (event.kind === 'end') {
            turn.status = event.status
            turn.errorCode = event.error?.code ?? null
            if (event.status === 'completed' && !request.retryTurnId) stored.summary.selectedTurnId = id
          }
          emit(event)
        }
        runs.push({ request, turnId: id, emit: wrapped, signal: options?.signal })
        options?.signal?.addEventListener('abort', () => wrapped({ kind: 'end', status: 'cancelled' }))
        return Promise.resolve({
          runId: `r${runs.length}`,
          conversationId: withHistory ? (request.conversationId ?? 'c1') : null,
          turnId: withHistory ? id : null,
        })
      },
      approve,
      ...extra.chat,
    },
  })
  if (ports.chat?.history)
    Object.assign(
      ports.chat.history,
      {
        get: (id: string) =>
          id === stored.summary.id
            ? Promise.resolve(structuredClone(stored))
            : Promise.reject(new KitClientError({ code: 'not_found', message: 'conversation not found' })),
      },
      extra.history,
    )
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
    expect(second?.request).toMatchObject({
      regenerate: false,
      retryTurnId: runs[0]?.turnId,
      conversationId: 'c1',
      model,
    })
    second?.emit({ kind: 'text_delta', text: 'Again' })
    second?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('Again')
    expect(screen.getByText('Stopped')).toBeTruthy()
    expect(screen.getByText('there')).toBeTruthy()
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
    expect(screen.getAllByText('hello')).toHaveLength(1)
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
    expect(runs[1]?.request).toMatchObject({ regenerate: false, retryTurnId: runs[0]?.turnId, conversationId: 'c1' })
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

  it('reloads model capabilities after providers settings change', async () => {
    const option = {
      ...model,
      label: 'Local · main · m1',
      modelType: 'chat',
      name: null,
      supportsTools: null,
      reasoning: null,
      nativeWebSearch: false,
    } as const
    const modelOptions = vi
      .fn<ProvidersPort['modelOptions']>()
      .mockResolvedValueOnce([{ ...option, canVision: false }])
      .mockResolvedValue([{ ...option, canVision: true }])
    setup(false, null, { providers: { modelOptions } })
    await fireEvent.click(await screen.findByLabelText('Select a model'))
    expect(screen.queryByRole('button', { name: 'Vision' })).toBeNull()
    await fireEvent.keyDown(screen.getByPlaceholderText('Search models…'), { key: 'Escape' })
    providersRevision.value++
    await waitFor(() => expect(modelOptions).toHaveBeenCalledTimes(2))
    await fireEvent.click(await screen.findByLabelText('Select a model'))
    expect(await screen.findByRole('button', { name: 'Vision' })).toBeTruthy()
  })

  it('keeps the chosen effort across a provider save and drops generate image once tools are unsupported', async () => {
    const option: ModelOption = {
      ...model,
      label: 'Local · main · m1',
      name: 'Mine',
      modelType: 'chat',
      supportsTools: null,
      reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
      nativeWebSearch: false,
      canVision: false,
    }
    const modelOptions = vi
      .fn<ProvidersPort['modelOptions']>()
      .mockResolvedValueOnce([option])
      .mockResolvedValue([{ ...option, supportsTools: false }])
    const { runs } = setup(false, null, {
      providers: { modelOptions },
      props: { paintingModel: true, toolbar: ['generate-image'] },
    })
    expect(await screen.findByText('Mine')).toBeTruthy()
    await fireEvent.click(await screen.findByRole('button', { name: 'Reasoning effort: Default' }))
    await fireEvent.input(await screen.findByRole('slider', { name: 'Reasoning effort' }), { target: { value: '1' } })
    await fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' })
    const generate = screen.getByRole('button', { name: 'Generate image' })
    await fireEvent.click(generate)
    expect(generate.getAttribute('aria-pressed')).toBe('true')
    providersRevision.value++
    await waitFor(() => expect(modelOptions).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(generate.hasAttribute('disabled')).toBe(true))
    expect(generate.getAttribute('aria-pressed')).toBe('false')
    expect(screen.getByRole('button', { name: 'Reasoning effort: High' })).toBeTruthy()
    await send('after save')
    expect(runs[0]?.request).toMatchObject({ reasoningEffort: 'high' })
    expect(runs[0]?.request.generateImage).toBeUndefined()
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
  it('customizes the toolbar: toggles, keyboard reorder, restore default (docs/38)', async () => {
    const onToolbarChange = vi.fn()
    setup(false, null, { search: {}, props: { onToolbarChange } })
    const toolbarLabels = () =>
      [...document.querySelectorAll('.composer-tools .tool-group:first-child > button')].map((b) =>
        b.getAttribute('aria-label'),
      )
    expect(toolbarLabels()).toEqual(['New conversation', 'Web search', 'More tools'])
    await fireEvent.click(screen.getByRole('button', { name: 'More tools' }))
    expect(await screen.findByRole('button', { name: /^Upload attachment/ })).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Web search' })).toHaveLength(1)
    await fireEvent.click(screen.getByRole('button', { name: 'Customize toolbar' }))
    await fireEvent.click(await screen.findByRole('switch', { name: 'Upload attachment' }))
    expect(onToolbarChange).toHaveBeenLastCalledWith(['new-conversation', 'web-search', 'attachment'])
    await fireEvent.keyDown(screen.getByRole('button', { name: 'Drag or use ↑/↓ to reorder Upload attachment' }), {
      key: 'ArrowUp',
    })
    expect(onToolbarChange).toHaveBeenLastCalledWith(['new-conversation', 'attachment', 'web-search'])
    await fireEvent.click(screen.getByRole('switch', { name: 'New conversation' }))
    expect(onToolbarChange).toHaveBeenLastCalledWith(['attachment', 'web-search'])
    expect(toolbarLabels()).toEqual(['Upload attachment', 'Web search', 'More tools'])
    await fireEvent.click(screen.getByRole('button', { name: 'Restore default' }))
    expect(onToolbarChange).toHaveBeenLastCalledWith(['new-conversation', 'web-search'])
    expect(screen.getByRole('button', { name: 'Restore default' }).hasAttribute('disabled')).toBe(true)
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
      cost: null,
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
    runs[1]?.emit({
      kind: 'usage',
      ...usage,
      inputTokens: 200,
      outputTokens: 40,
      cost: { amount: 0.0123, currency: 'USD' },
    })
    runs[1]?.emit({ kind: 'context', usage: { tokens: 240, contextWindow: 1000, source: 'reported', ...model } })
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(screen.getByRole('meter').getAttribute('value')).toBe('24'))
    const inputRow = screen.getByRole('row', { name: 'Input 200 320' })
    expect(inputRow).toBeTruthy()
    expect(screen.getByRole('row', { name: 'Cost $0.0123 $0.0123' })).toBeTruthy()
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Regenerate' }).at(-1)))
    await waitFor(() => expect(screen.queryByRole('meter')).toBeNull())
    expect(screen.queryByRole('meter')).toBeNull()
  })

  it('restores reasoning and statistics from history without inventing unknown fields', async () => {
    setup(true, null, {
      props: { conversationId: 'saved' },
      history: {
        get: () =>
          Promise.resolve({
            summary: { id: 'saved', title: 'Saved', scope: null, selectedTurnId: 't', createdAt: 0, updatedAt: 0 },
            turns: [
              {
                id: 't',
                parentId: null,
                questionId: 't',
                supersedesId: null,
                kind: 'reply',
                status: 'completed',
                input: 'question',
                text: 'answer',
                errorCode: null,
                ...model,
                reasoningEffort: 'high',
                reasoningText: 'summary',
                images: [],
                generated: [],
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
        : Promise.resolve({ runId: 'r2', conversationId: null, turnId: null })
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
    const summary = () => ({ id: 'c9', title: saved, scope: null, selectedTurnId: 't', createdAt: 0, updatedAt: 0 })
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

  it('offers every eligible reply action and reports clipboard and insertion failures', async () => {
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
    expect(screen.getAllByRole('button', { name: 'Regenerate' })).toHaveLength(2)
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
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['high'] },
              nativeWebSearch: false,
              canVision: false,
            },
          ]),
      },
    })
    const effort = await screen.findByRole('button', { name: 'Reasoning effort: Default' })
    await fireEvent.click(effort)
    expect(await screen.findByLabelText('High')).toBeTruthy()
    expect(screen.queryByLabelText('Medium')).toBeNull()
    expect(screen.queryByRole('slider')).toBeNull()
    expect((screen.getByLabelText('Default') as HTMLInputElement).checked).toBe(true)
    await send('default request')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
    await waitFor(() => expect(effort.hasAttribute('disabled')).toBe(true))
    expect((screen.getByLabelText('High') as HTMLInputElement).matches(':disabled')).toBe(true)
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(effort.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(screen.getByLabelText('High'))
    await send('explicit request')
    expect(runs[1]?.request.reasoningEffort).toBe('high')
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Reset to default' }).hasAttribute('disabled')).toBe(false),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Reset to default' }))
    await send('reset request')
    expect(runs[2]?.request.reasoningEffort).toBe('default')
  })

  it('maps sparse slider indices exactly, preserves default on open, and isolates Escape', async () => {
    const onClose = vi.fn()
    const { runs } = setup(false, null, {
      props: { onClose },
      providers: {
        modelOptions: () =>
          Promise.resolve([
            {
              ...model,
              label: 'Sparse reasoner',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['high', 'none', 'low'] },
              nativeWebSearch: false,
              canVision: false,
            },
            {
              endpointId: model.endpointId,
              modelId: 'm2',
              label: 'Other reasoner',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
              nativeWebSearch: false,
              canVision: false,
            },
            {
              endpointId: model.endpointId,
              modelId: 'm3',
              label: 'Without reasoning',
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
    const trigger = await screen.findByRole('button', { name: 'Reasoning effort: Default' })
    await fireEvent.click(trigger)
    let slider = (await screen.findByRole('slider', { name: 'Reasoning effort' })) as HTMLInputElement
    expect(slider.value).toBe('1')
    expect(slider.max).toBe('2')
    expect(slider.getAttribute('aria-valuetext')).toBe('Default')
    await fireEvent.keyDown(slider, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('slider')).toBeNull())
    expect(onClose).not.toHaveBeenCalled()
    await waitFor(() => expect(document.activeElement).toBe(trigger))
    await send('opened default')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(trigger.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(trigger)
    slider = (await screen.findByRole('slider', { name: 'Reasoning effort' })) as HTMLInputElement
    for (const [index, effort] of ['none', 'low', 'high'].entries()) {
      await fireEvent.input(slider, { target: { value: String(index) } })
      await send(`effort ${effort}`)
      expect(runs[index + 1]?.request.reasoningEffort).toBe(effort)
      await waitFor(() => expect(slider.disabled).toBe(true))
      runs[index + 1]?.emit({ kind: 'end', status: 'completed' })
      await waitFor(() => expect(slider.disabled).toBe(false))
    }
    await fireEvent.keyDown(slider, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('slider')).toBeNull())
    const models = screen.getByLabelText('Select a model')
    await fireEvent.click(models)
    await fireEvent.click(await screen.findByRole('option', { name: /m2/ }))
    expect(await screen.findByRole('button', { name: 'Reasoning effort: Default' })).toBeTruthy()
    await send('changed model')
    expect(runs[4]?.request).toMatchObject({ model: { endpointId: model.endpointId, modelId: 'm2' } })
    expect(runs[4]?.request.reasoningEffort).toBe('default')
  })

  it('groups models by provider, searches, filters by capability and links to settings', async () => {
    const onOpenSettings = vi.fn()
    const onClose = vi.fn()
    const base = {
      name: null,
      modelType: 'chat',
      supportsTools: null,
      reasoning: null,
      nativeWebSearch: false,
      canVision: false,
    } as const
    setup(false, null, {
      props: { onOpenSettings, onClose },
      providers: {
        modelOptions: () =>
          Promise.resolve([
            { ...base, ...model, label: 'Local · main · m1', supportsTools: true },
            {
              ...base,
              endpointId: 'e2',
              modelId: 'm2',
              label: 'Cloud · x · m2',
              reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
              canVision: true,
            },
          ]),
      },
    })
    await fireEvent.click(await screen.findByLabelText('Select a model'))
    expect(await screen.findByText('Local · main')).toBeTruthy()
    expect(screen.getByText('Cloud · x')).toBeTruthy()
    expect(screen.getAllByRole('option')).toHaveLength(2)
    await fireEvent.input(screen.getByPlaceholderText('Search models…'), { target: { value: 'cloud' } })
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([expect.stringContaining('m2')]),
    )
    await fireEvent.keyDown(screen.getByPlaceholderText('Search models…'), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('option')).toBeNull())
    expect(onClose).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByLabelText('Select a model'))
    await screen.findAllByRole('option')
    await fireEvent.click(screen.getByRole('button', { name: 'Vision' }))
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([expect.stringContaining('m2')]),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Vision' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Tools' }))
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([expect.stringContaining('m1')]),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Configure models' }))
    expect(onOpenSettings).toHaveBeenCalledOnce()
    await waitFor(() => expect(screen.queryByRole('option')).toBeNull())
  })

  it('keeps reasoning discoverable without sending unsupported efforts', async () => {
    const onOpenSettings = vi.fn()
    const { runs } = setup(false, null, { props: { onOpenSettings } })
    await fireEvent.click(await screen.findByRole('button', { name: 'Reasoning effort: Default' }))
    expect(await screen.findByText(/No reasoning capabilities are configured/)).toBeTruthy()
    expect(screen.queryByRole('slider')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Open settings' }))
    expect(onOpenSettings).toHaveBeenCalledOnce()
    await send('provider default')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
  })
  it('retries a failed history restore without losing the saved conversation or replacing its identity', async () => {
    const onConversationChange = vi.fn()
    const get = vi
      .fn<ChatHistoryPort['get']>()
      .mockRejectedValueOnce(new KitClientError({ code: 'network', message: 'offline' }))
      .mockResolvedValueOnce({
        summary: { id: 'saved', title: 'Saved', scope: null, selectedTurnId: 't', createdAt: 0, updatedAt: 0 },
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
    expect(screen.queryByRole('button', { name: 'Save to note' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Set as context' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Delete' })).toBeNull()
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

  function candidates(): Conversation {
    const reply = (id: string, text: string): Conversation['turns'][number] => ({
      id,
      parentId: null,
      questionId: 'q',
      supersedesId: null,
      kind: 'reply',
      ...model,
      input: 'Shared question',
      text,
      status: 'completed',
      errorCode: null,
      reasoningEffort: 'default',
      reasoningText: null,
      images: [],
      generated: [],
    })
    return {
      summary: { id: 'saved', title: 'Candidates', scope: null, selectedTurnId: 'a', createdAt: 0, updatedAt: 0 },
      turns: [reply('a', 'Original **reply**'), reply('b', 'Alternative reply')],
    }
  }

  it('renders full same-question candidates once and sets context via 👍 without forking', async () => {
    const conversation = candidates()
    const selectTurn = vi.fn<ChatHistoryPort['selectTurn']>((_id, turnId) =>
      Promise.resolve({ ...conversation, summary: { ...conversation.summary, selectedTurnId: turnId } }),
    )
    const fork = vi.fn<ChatHistoryPort['fork']>(() => Promise.resolve(conversation))
    const { runs } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), selectTurn, fork },
    })
    await screen.findByText('Alternative reply')
    expect(screen.getAllByText('Shared question')).toHaveLength(1)
    expect(screen.getByText('reply', { selector: 'strong' })).toBeTruthy()
    expect(screen.getAllByText('m1', { selector: '.reply-source' })).toHaveLength(2)
    const setContext = screen.getAllByRole('button', { name: 'Set as context' })
    expect(setContext.map((button) => button.getAttribute('aria-pressed'))).toEqual(['true', 'false'])
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Keep my draft' } })
    await fireEvent.click(required(setContext[1]))
    await waitFor(() => expect(selectTurn).toHaveBeenCalledWith('saved', 'b'))
    expect(fork).not.toHaveBeenCalled()
    expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('Keep my draft')
    await send('Continue chosen reply')
    expect(runs[0]?.request).toMatchObject({ conversationId: 'saved', replyToId: 'b' })
  })

  it('edits with compare-and-swap and retains both saved reply and local draft on conflict', async () => {
    const conversation = candidates()
    const editTurn = vi
      .fn<ChatHistoryPort['editTurn']>()
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
    const editor = (await screen.findByRole('textbox', { name: 'Reply Markdown' })) as HTMLTextAreaElement
    await fireEvent.input(editor, { target: { value: 'Edited local draft' } })
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    await screen.findByText('Conflicts with existing data')
    expect(editor.value).toBe('Edited local draft')
    expect(screen.getByText('Alternative reply')).toBeTruthy()
    expect(editTurn).toHaveBeenLastCalledWith('saved', 'b', 'Edited local draft', 'Alternative reply')
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
    expect(runs[0]?.request).toMatchObject({ conversationId: 'saved', retryTurnId: 'a', model, regenerate: false })
    expect(screen.getByText('reply', { selector: 'strong' })).toBeTruthy()
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: 'Switch model' })[1]?.hasAttribute('disabled')).toBe(false),
    )
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Switch model' })[1]))
    expect(screen.queryByRole('dialog')).toBeNull()
    await fireEvent.click(await screen.findByRole('option', { name: /m2/ }))
    expect(runs[1]?.request).toMatchObject({ conversationId: 'saved', retryTurnId: 'b', model: secondModel })
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
    const removeTurn = vi.fn<ChatHistoryPort['removeTurn']>(() =>
      Promise.resolve({ ...conversation, summary: { ...conversation.summary, selectedTurnId: null }, turns: [] }),
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
    expect(notify.confirm).toHaveBeenLastCalledWith({ title: 'Delete this message?', body: '', danger: true })
    await waitFor(() => expect(screen.queryByText('Shared question')).toBeNull())
  })

  it('branches and copies via the selected ancestor chain and only shows save-note for a host callback', async () => {
    const conversation = candidates()
    const fork = vi.fn<ChatHistoryPort['fork']>(() =>
      Promise.resolve({ ...conversation, summary: { ...conversation.summary, id: 'forked' } }),
    )
    const selectTurn = vi.fn<ChatHistoryPort['selectTurn']>((_id, turnId) =>
      Promise.resolve({ ...conversation, summary: { ...conversation.summary, selectedTurnId: turnId } }),
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
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Could not save the note'))
    expect(onSaveNote).toHaveBeenCalledWith('Alternative reply', [])
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Discard for branch' } })
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

  it('downloads real Markdown blobs and reports download failures', async () => {
    const conversation = candidates()
    const createObjectURL = vi.fn<(blob: Blob) => string>(() => 'blob:markdown')
    const revokeObjectURL = vi.fn()
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: createObjectURL })
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revokeObjectURL })
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined)
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Save' }))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Save to local file' }))
    expect(createObjectURL).toHaveBeenCalledWith(expect.any(Blob))
    const blob = createObjectURL.mock.calls[0]?.[0] as Blob | undefined
    expect(blob?.type).toBe('text/markdown;charset=utf-8')
    const fileReader = new FileReader()
    const markdown = new Promise<string>((resolve, reject) => {
      fileReader.onload = () => resolve(String(fileReader.result))
      fileReader.onerror = () => reject(fileReader.error)
    })
    fileReader.readAsText(required(blob))
    expect(await markdown).toBe('Alternative reply')
    expect(click).toHaveBeenCalledOnce()
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:markdown')
    createObjectURL.mockImplementationOnce(() => {
      throw new Error('unavailable')
    })
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Export' }))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Export as Markdown' }))
    expect(notify.toast).toHaveBeenCalledWith('error', 'Could not download Markdown')
    click.mockRestore()
  })

  it('shows an unavailable source model instead of silently using the composer model', async () => {
    const conversation = candidates()
    required(conversation.turns[1]).endpointId = 'removed-endpoint'
    required(conversation.turns[1]).modelId = 'removed-model'
    const { runs, notify } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Regenerate' })[1]))
    expect(notify.toast).toHaveBeenCalledWith('error', 'The source model is unavailable. Use @ to pick another model.')
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(runs).toHaveLength(0)
    expect(screen.getByText('Alternative reply')).toBeTruthy()
  })

  it('keeps deletion of a reply ancestor enabled with a reparent warning but disables editing it', async () => {
    const conversation = candidates()
    conversation.summary.selectedTurnId = 'b'
    required(conversation.turns[1]).parentId = 'a'
    const removeTurn = vi.fn<ChatHistoryPort['removeTurn']>(() =>
      Promise.resolve({ ...conversation, turns: [{ ...required(conversation.turns[1]), parentId: null }] }),
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
      expect(screen.getAllByRole('button', { name: label }).every((button) => button.hasAttribute('disabled'))).toBe(
        true,
      )
    }
  })

  it('retains translation language and original candidates when acceptance fails', async () => {
    const conversation = candidates()
    const run = vi.fn<ChatPort['run']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
      chat: { run },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Translate' })[1]))
    const language = (await screen.findByRole('textbox', { name: 'Target language' })) as HTMLInputElement
    await fireEvent.input(language, { target: { value: '日本語' } })
    await fireEvent.click(screen.getByRole('button', { name: 'OK' }))
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('Network connection failed'))
    expect(language.value).toBe('日本語')
    expect(screen.getByText('Alternative reply')).toBeTruthy()
    expect(screen.getByText('reply', { selector: 'strong' })).toBeTruthy()
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
  })

  it('keeps branch drafts and selection unchanged when context persistence rejects', async () => {
    const conversation = candidates()
    const selectTurn = vi.fn<ChatHistoryPort['selectTurn']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation), selectTurn },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Keep on failure' } })
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Branch' }))
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Network connection failed'))
    expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('Keep on failure')
    expect(screen.getAllByRole('button', { name: 'Set as context' })[0]?.getAttribute('aria-pressed')).toBe('true')
  })

  it('awaits native Markdown export and reports host failure without using a browser download', async () => {
    const conversation = candidates()
    const onExportMarkdown = vi.fn(() => Promise.reject(new Error('disk full')))
    const { notify } = setup(true, null, {
      props: { conversationId: 'saved', onExportMarkdown },
      history: { get: () => Promise.resolve(conversation) },
    })
    await screen.findByText('Alternative reply')
    await fireEvent.click(required(screen.getAllByLabelText('More actions')[1]))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Export' }))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Export as Markdown' }))
    await waitFor(() => expect(onExportMarkdown).toHaveBeenCalledWith('Alternative reply'))
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Could not download Markdown'))
    expect(screen.getByText('Alternative reply')).toBeTruthy()
  })
})

describe('ChatPanel images', () => {
  const visionModel = { endpointId: 'e2', modelId: 'v1' }
  const option = (ref: typeof model, label: string, canVision: boolean): ModelOption => ({
    ...ref,
    label,
    name: null,
    modelType: 'chat',
    supportsTools: null,
    reasoning: null,
    nativeWebSearch: false,
    canVision,
  })
  const textOption = option(model, 'Text model', false)
  const visionOption = option(visionModel, 'Vision model', true)
  const png = (name = 'a.png', size = 3) => new File([new Uint8Array(size).fill(65)], name, { type: 'image/png' })
  const revoke = vi.fn()
  let created = 0
  beforeEach(() => {
    created = 0
    revoke.mockClear()
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: () => `blob:image-${++created}` })
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revoke })
  })
  function visionSetup(extra: Extra = {}) {
    return setup(true, null, {
      ...extra,
      props: { model: visionModel, ...extra.props },
      providers: { modelOptions: () => Promise.resolve([textOption, visionOption]), ...extra.providers },
    })
  }
  async function uploadItem() {
    await fireEvent.click(screen.getByRole('button', { name: 'More tools' }))
    return screen.findByRole('button', { name: /^Upload attachment/ })
  }
  async function pick(files: File[]) {
    const upload = await uploadItem()
    await waitFor(() => expect(upload.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(upload)
    await waitFor(() => expect(screen.queryByRole('button', { name: /^Upload attachment/ })).toBeNull())
    const input = required(document.querySelector<HTMLInputElement>('input[type=file]'))
    expect(input.accept).toBe('image/png,image/jpeg,image/webp,image/gif')
    await fireEvent.change(input, { target: { files } })
  }

  it('attaches, removes and sends images, then clears the tray and shows them on the turn', async () => {
    const { runs } = visionSetup()
    await pick([png('a.png'), png('b.png')])
    await fireEvent.click(await screen.findByRole('button', { name: 'Remove image 2' }))
    expect(revoke).toHaveBeenCalledWith('blob:image-2')
    expect(screen.queryByRole('button', { name: 'Remove image 2' })).toBeNull()
    await send('What is this?')
    expect(runs[0]?.request.images).toEqual([{ mime: 'image/png', data: 'QUFB' }])
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Remove image 1' })).toBeNull())
    const link = screen.getByRole('link', { name: 'Open image 1' })
    expect(link.getAttribute('target')).toBe('_blank')
    expect(link.getAttribute('rel')).toBe('noopener')
    expect(link.getAttribute('href')).toBe('data:image/png;base64,QUFB')
    expect(revoke).toHaveBeenCalledWith('blob:image-1')
  })

  it('takes pasted images and leaves plain text paste alone', async () => {
    const { runs } = visionSetup()
    const input = screen.getByRole('textbox', { name: 'Message' })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy())
    expect(await fireEvent.paste(input, { clipboardData: { files: [] } })).toBe(true)
    expect(await fireEvent.paste(input, { clipboardData: { files: [png()] } })).toBe(false)
    await screen.findByRole('button', { name: 'Remove image 1' })
    await send('pasted')
    expect(runs[0]?.request.images).toHaveLength(1)
  })

  it('accepts dropped images and rejects over-limit, wrong type and oversized files without queueing', async () => {
    const { notify } = visionSetup()
    await pick([png('1.png'), png('2.png'), png('3.png')])
    await screen.findByRole('button', { name: 'Remove image 3' })
    const form = required(screen.getByRole('textbox', { name: 'Message' }).closest('form'))
    await fireEvent.drop(form, { dataTransfer: { files: [png('4.png'), png('5.png')], types: ['Files'] } })
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Up to 4 images per message'))
    await fireEvent.drop(form, { dataTransfer: { files: [new File(['x'], 'x.bmp', { type: 'image/bmp' })] } })
    expect(notify.toast).toHaveBeenCalledWith('error', 'Only PNG, JPEG, WebP and GIF images are supported')
    await fireEvent.drop(form, { dataTransfer: { files: [png('big.png', 4 * 1024 * 1024 + 1)] } })
    expect(notify.toast).toHaveBeenCalledWith('error', 'Each image must be 4 MiB or smaller')
    expect(screen.queryByRole('button', { name: 'Remove image 4' })).toBeNull()
    await fireEvent.drop(form, { dataTransfer: { files: [png('4.png')] } })
    await screen.findByRole('button', { name: 'Remove image 4' })
  })

  it('disables attach and send for a non-vision model until a vision model is picked', async () => {
    const { notify } = visionSetup({ props: { model } })
    const upload = await uploadItem()
    await waitFor(() => expect(upload.getAttribute('title')).toBe('The current model does not support images'))
    expect(upload.hasAttribute('disabled')).toBe(true)
    await fireEvent.keyDown(upload, { key: 'Escape' })
    await fireEvent.paste(screen.getByRole('textbox', { name: 'Message' }), { clipboardData: { files: [png()] } })
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'The current model does not support images'))
    expect(screen.queryByRole('button', { name: 'Remove image 1' })).toBeNull()
    await fireEvent.click(screen.getByLabelText('Select a model'))
    await fireEvent.click(await screen.findByRole('option', { name: /v1/ }))
    await pick([png()])
    await screen.findByRole('button', { name: 'Remove image 1' })
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'hi' } })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(false))
    await fireEvent.click(screen.getByLabelText('Select a model'))
    await fireEvent.click(await screen.findByRole('option', { name: /m1/ }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(true))
    expect(screen.getByRole('status', { name: '' }).textContent).toContain('does not support images')
  })

  it('restores images with the draft when the request is rejected before acceptance', async () => {
    const run = vi
      .fn<ChatPort['run']>()
      .mockRejectedValueOnce(new KitClientError({ code: 'network', message: 'offline' }))
    visionSetup({ chat: { run } })
    await pick([png()])
    await screen.findByRole('button', { name: 'Remove image 1' })
    await send('keep my image')
    expect(run.mock.calls[0]?.[0].images).toEqual([{ mime: 'image/png', data: 'QUFB' }])
    await waitFor(() =>
      expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('keep my image'),
    )
    expect(screen.getByRole('button', { name: 'Remove image 1' })).toBeTruthy()
    expect(revoke).not.toHaveBeenCalled()
  })

  it('shows expired history images and lists only vision models for @ when the source has images', async () => {
    const turn = (id: string): Conversation['turns'][number] => ({
      id,
      parentId: null,
      questionId: 'q',
      supersedesId: null,
      kind: 'reply',
      ...model,
      input: 'Shared question',
      text: `Reply ${id}`,
      status: 'completed',
      errorCode: null,
      reasoningEffort: 'default',
      reasoningText: null,
      images: [
        { mime: 'image/png', url: null },
        { mime: 'image/png', url: '/img/1' },
      ],
      generated: [],
    })
    const conversation: Conversation = {
      summary: { id: 'saved', title: 'Images', scope: null, selectedTurnId: 'a', createdAt: 0, updatedAt: 0 },
      turns: [turn('a'), turn('b')],
    }
    visionSetup({ props: { conversationId: 'saved' }, history: { get: () => Promise.resolve(conversation) } })
    expect(await screen.findByText('Image expired')).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Open image 2' }).getAttribute('href')).toBe('/img/1')
    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: 'Switch model' })[0]?.hasAttribute('disabled')).toBe(false),
    )
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Switch model' })[0]))
    await screen.findByRole('option', { name: /v1/ })
    expect(screen.queryByRole('option', { name: /m1/ })).toBeNull()
  })

  it('saves an image-only reply to a note together with its generated images', async () => {
    const generated = [{ mime: 'image/png', url: 'data:image/png;base64,AAAA' }]
    const conversation: Conversation = {
      summary: { id: 'saved', title: 'Painting', scope: null, selectedTurnId: 'a', createdAt: 0, updatedAt: 0 },
      turns: [
        {
          id: 'a',
          parentId: null,
          questionId: 'q',
          supersedesId: null,
          kind: 'reply',
          ...model,
          input: 'Draw a cat',
          text: '',
          status: 'completed',
          errorCode: null,
          reasoningEffort: 'default',
          reasoningText: null,
          images: [],
          generated,
        },
      ],
    }
    const onSaveNote = vi.fn()
    visionSetup({
      props: { conversationId: 'saved', onSaveNote },
      history: { get: () => Promise.resolve(conversation) },
    })
    await fireEvent.click(await screen.findByRole('button', { name: 'Save to note' }))
    expect(onSaveNote).toHaveBeenCalledWith('', generated)
  })
})
