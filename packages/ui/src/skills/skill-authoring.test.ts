import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError, type SkillAuthoringPort, type RunEvent, type Source } from '../ports'
import {
  authorMessage,
  authoring,
  creator,
  mountAuthoring,
  preview,
  selectAuthoringModel,
  startAuthoring,
} from './test-support/skills-fixture'

describe('Skill authoring', () => {
  it('reviews the complete built-in creator before creating an authoring session', async () => {
    const api = authoring()
    mountAuthoring(api)
    await fireEvent.click(screen.getByRole('button', { name: 'Add skill' }))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Create with Agent' }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Review skill-creator' }))
    await screen.findByText(creator.body)
    expect(api.create).not.toHaveBeenCalled()
    expect(screen.getByText('scripts/quick_validate.py')).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm skill-creator and start' }))
    await screen.findByLabelText('Message to the skill creation assistant')
    expect(api.create).toHaveBeenCalledWith({ signal: expect.any(AbortSignal) })
  })

  it('keeps failed authoring output unsaved and permits another turn', async () => {
    const send = vi.fn<SkillAuthoringPort['send']>(async (_id, _input, _model, emit) => {
      emit({ kind: 'text_delta', text: 'partial work' })
      emit({
        kind: 'end',
        status: 'failed',
        error: { code: 'needs_config', message: 'not configured' },
      })
      return { runId: 'failed-run', conversationId: null, turnId: null }
    })
    const api = authoring({ send })
    const save = vi.fn()
    const install = vi.fn()
    const previewFn = vi.fn()
    mountAuthoring(api, { skills: { save, install, preview: previewFn } })
    await startAuthoring()
    await selectAuthoringModel()
    await authorMessage('Create a summary skill')
    await screen.findByRole('alert')
    expect(screen.getByText('partial work')).toBeTruthy()
    expect(
      screen.getByLabelText('Message to the skill creation assistant').hasAttribute('disabled'),
    ).toBe(false)
    expect(save).not.toHaveBeenCalled()
    expect(previewFn).not.toHaveBeenCalled()
    expect(install).not.toHaveBeenCalled()
    expect(api.prepare).not.toHaveBeenCalled()
  })

  it('continues multiple turns and previews the prepared package before any install', async () => {
    const send = vi.fn<SkillAuthoringPort['send']>(async (_id, input, _model, emit) => {
      emit({ kind: 'text_delta', text: `Assistant: ${input}` })
      emit({ kind: 'end', status: 'completed' })
      return { runId: 'complete-run', conversationId: null, turnId: null }
    })
    const source: Source = { bytes: { fileName: 'complete-skill.zip', bytes: [1, 2, 3] } }
    const prepare = vi.fn(async () => ({ source, origin: 'Authoring workspace' }))
    const api = authoring({ send, prepare })
    const previewFn = vi.fn(async () => preview('packaged skill body'))
    const save = vi.fn()
    const install = vi.fn()
    mountAuthoring(api, { skills: { preview: previewFn, save, install } })
    await startAuthoring()
    await selectAuthoringModel()
    await authorMessage('Create a writer')
    await screen.findByText('Assistant: Create a writer')
    await authorMessage('Add a validation script')
    await screen.findByText('Assistant: Add a validation script')
    expect(send).toHaveBeenNthCalledWith(
      2,
      'authoring-1',
      'Add a validation script',
      { endpointId: 'e1', modelId: 'm1' },
      expect.any(Function),
      { signal: expect.any(AbortSignal) },
    )
    expect(prepare).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Preview artifact' }))
    await screen.findByText('packaged skill body')
    expect(prepare).toHaveBeenCalledWith('authoring-1', { signal: expect.any(AbortSignal) })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(save).not.toHaveBeenCalled()
    expect(install).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    await screen.findByLabelText('Message to the skill creation assistant')
    expect(screen.getByText('Assistant: Add a validation script')).toBeTruthy()
    expect(api.close).not.toHaveBeenCalled()
  })

  it('shows argument previews and routes explicit allow and deny decisions to the authoring run', async () => {
    const args = '{"command":"python scripts/quick_validate.py output/writer","cwd":"workspace"}'
    let emit: ((event: RunEvent) => void) | undefined
    const api = authoring({
      send: vi.fn<SkillAuthoringPort['send']>(async (_id, _input, _model, receive) => {
        emit = receive
        receive({ kind: 'tool_call', callId: 'call-1', name: 'shell', source: 'host' })
        receive({ kind: 'approval_required', callId: 'call-1', name: 'shell', argsPreview: args })
        return { runId: 'approval-run', conversationId: null, turnId: null }
      }),
    })
    mountAuthoring(api)
    await startAuthoring()
    await selectAuthoringModel()
    await authorMessage('Test the skill')
    await screen.findByText(args)
    expect(screen.getByText(/Arguments preview/).textContent).toContain('not sandboxed')
    await fireEvent.click(await screen.findByRole('button', { name: 'Allow once' }))
    await waitFor(() =>
      expect(api.approve).toHaveBeenCalledWith('authoring-1', 'approval-run', 'call-1', true),
    )
    emit?.({ kind: 'tool_result', callId: 'call-1', outcome: 'ok', summary: 'validation passed' })
    await screen.findByText('shell: validation passed')
    emit?.({ kind: 'tool_call', callId: 'call-2', name: 'shell', source: 'host' })
    emit?.({
      kind: 'approval_required',
      callId: 'call-2',
      name: 'shell',
      argsPreview: '{"command":"unsafe"}',
    })
    await fireEvent.click(await screen.findByRole('button', { name: 'Deny' }))
    await waitFor(() =>
      expect(api.approve).toHaveBeenCalledWith('authoring-1', 'approval-run', 'call-2', false),
    )
  })

  it('cancels and closes authoring workspaces and ignores late run acceptance/output', async () => {
    const acceptance = Promise.withResolvers<{
      runId: string
      conversationId: null
      turnId: null
    }>()
    let emit: ((event: RunEvent) => void) | undefined
    const send = vi.fn<SkillAuthoringPort['send']>((_id, _input, _model, onEvent) => {
      emit = onEvent
      return acceptance.promise
    })
    const api = authoring({ send })
    mountAuthoring(api)
    await startAuthoring()
    await selectAuthoringModel()
    await authorMessage('Create a writer')
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    await waitFor(() => expect(api.close).toHaveBeenCalledWith('authoring-1'))
    expect(send.mock.calls[0]?.[4]?.signal?.aborted).toBe(true)
    acceptance.resolve({ runId: 'late-run', conversationId: null, turnId: null })
    emit?.({ kind: 'text_delta', text: 'late text' })
    emit?.({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(api.cancel).toHaveBeenCalledWith('authoring-1', 'late-run'))
    expect(screen.queryByText('late text')).toBeNull()
  })

  it('closes a session created after its confirmation dialog was closed', async () => {
    const pending = Promise.withResolvers<string>()
    const api = authoring({ create: vi.fn(() => pending.promise) })
    mountAuthoring(api)
    await fireEvent.click(screen.getByRole('button', { name: 'Add skill' }))
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Create with Agent' }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Review skill-creator' }))
    await screen.findByText(creator.body)
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm skill-creator and start' }))
    await fireEvent.click(await screen.findByRole('button', { name: /^Cancel$/ }))
    pending.resolve('late-session')
    await waitFor(() => expect(api.close).toHaveBeenCalledWith('late-session'))
    expect(api.send).not.toHaveBeenCalled()
    expect(screen.queryByLabelText('Message to the skill creation assistant')).toBeNull()
  })

  it('cancels an accepted authoring run before closing its session on unmount', async () => {
    const api = authoring({
      send: vi.fn(async () => ({ runId: 'active-run', conversationId: null, turnId: null })),
    })
    const view = mountAuthoring(api)
    await startAuthoring()
    await selectAuthoringModel()
    await authorMessage('Create a writer')
    await view.unmount()
    await waitFor(() => expect(api.close).toHaveBeenCalledWith('authoring-1'))
    expect(api.cancel).toHaveBeenCalledWith('authoring-1', 'active-run')
    expect(vi.mocked(api.cancel).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(api.close).mock.invocationCallOrder[0] ?? 0,
    )
  })

  it('shows package preparation failure without losing the conversation or installing', async () => {
    const api = authoring({
      prepare: vi.fn(async () => {
        throw new KitClientError({ code: 'bad_request', message: 'no artifact' })
      }),
    })
    const install = vi.fn()
    mountAuthoring(api, { skills: { install } })
    await startAuthoring()
    await selectAuthoringModel()
    await authorMessage('Create a writer')
    await fireEvent.click(screen.getByRole('button', { name: 'Preview artifact' }))
    await screen.findByRole('alert')
    expect(screen.getByText('Create a writer')).toBeTruthy()
    expect(
      screen.getByLabelText('Message to the skill creation assistant').hasAttribute('disabled'),
    ).toBe(false)
    expect(install).not.toHaveBeenCalled()
  })

  it('requires an explicit authoring model without sending or saving', async () => {
    const api = authoring()
    const save = vi.fn()
    mountAuthoring(api, { skills: { save } })
    await startAuthoring()
    await authorMessage('Create a writer')
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Choose a configured chat model before sending',
    )
    expect(api.send).not.toHaveBeenCalled()
    expect(save).not.toHaveBeenCalled()
  })
})
