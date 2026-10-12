import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, type RunEvent, type RunRequest } from '../ports'
import Harness from './Harness.test.svelte'

const model = { endpointId: 'e1', modelId: 'm1' }

interface Run {
  request: RunRequest
  emit: (event: RunEvent) => void
  signal: AbortSignal | undefined
}

function setup(withHistory: boolean, context: { kind: string; id: string; label: string } | null = null) {
  const runs: Run[] = []
  const approve = vi.fn<(runId: string, callId: string, allow: boolean) => Promise<void>>(() => Promise.resolve())
  const ports = createFakePorts({
    providers: {
      modelOptions: () =>
        Promise.resolve([{ ...model, label: 'Local · main · m1', modelType: 'chat', supportsTools: null }]),
    },
    chat: {
      run: (request, emit, options) => {
        runs.push({ request, emit, signal: options?.signal })
        // Like the backend: aborting cancels, and the run ends once as `cancelled`.
        options?.signal?.addEventListener('abort', () => emit({ kind: 'end', status: 'cancelled' }))
        return Promise.resolve({ runId: `r${runs.length}`, conversationId: withHistory ? 'c1' : null })
      },
      approve,
    },
  })
  if (!withHistory && ports.chat) delete ports.chat.history
  const notify = { toast: vi.fn(), confirm: () => Promise.resolve(true) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const view = render(Harness, { props: { kit: { ports, i18n: createI18n('en'), notify, logger }, model, context } })
  return { runs, view, approve }
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
})
