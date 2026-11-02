import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError, type RunAccepted, type RunClient, type RunRequest } from '../ports'
import { model, required, setup, send } from './test-support/chat-fixture'

describe('ChatPanel runs', () => {
  it('sends the host context, drops it for one message when removed, then sends it again', async () => {
    const { runs } = setup(false, { kind: 'note', id: 'n1', label: 'Draft' })
    await send('one')
    expect(runs[0]?.request.context).toEqual({ kind: 'note', id: 'n1' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Do not send Draft with the next message' }),
    )
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
    run?.emit({
      kind: 'approval_required',
      callId: 'c2',
      name: 'write_note',
      argsPreview: '{"text":"x"}',
    })
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
    runs[0]?.emit({
      kind: 'context',
      usage: { tokens: 150, contextWindow: 1000, source: 'reported', ...model },
    })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(screen.getByRole('button', { name: 'Usage and settings' }))
    await waitFor(() => expect(screen.getByRole('meter').getAttribute('value')).toBe('15'))
    await fireEvent.click(screen.getByText('Token usage', { selector: 'summary' }))
    await send('second')
    runs[1]?.emit({
      kind: 'usage',
      ...usage,
      inputTokens: 200,
      outputTokens: 40,
      cost: { amount: 0.0123, currency: 'USD' },
    })
    runs[1]?.emit({
      kind: 'context',
      usage: { tokens: 240, contextWindow: 1000, source: 'reported', ...model },
    })
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(screen.getByRole('meter').getAttribute('value')).toBe('24'))
    const inputRow = screen.getByRole('row', { name: 'Input 200 320' })
    expect(inputRow).toBeTruthy()
    expect(screen.getByRole('row', { name: 'Cost $0.0123 $0.0123' })).toBeTruthy()
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Regenerate' }).at(-1)))
    await waitFor(() => expect(screen.queryByRole('meter')).toBeNull())
    expect(screen.queryByRole('meter')).toBeNull()
  })

  it('restores a preaccept-rejected draft and its removed context for the next request', async () => {
    const requests: RunRequest[] = []
    const run = vi.fn<RunClient['start']>().mockImplementation((request) => {
      requests.push(request)
      return requests.length === 1
        ? Promise.reject(new KitClientError({ code: 'network', message: 'offline' }))
        : Promise.resolve({ runId: 'r2', conversationId: null, turnId: null })
    })
    setup(false, { kind: 'note', id: 'n1', label: 'Draft' }, { chat: { start: run } })
    await fireEvent.click(
      screen.getByRole('button', { name: 'Do not send Draft with the next message' }),
    )
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
    setup(false, null, { chat: { start: () => pending.promise } })
    await send('unsent original')
    const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement
    await fireEvent.input(input, { target: { value: 'new draft' } })
    pending.reject(new KitClientError({ code: 'network', message: 'offline' }))
    await screen.findByRole('button', { name: 'Restore unsent message' })
    expect(input.value).toBe('new draft')
    expect(
      screen.getByRole('button', { name: 'Restore unsent message' }).hasAttribute('disabled'),
    ).toBe(true)
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
})
