import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError, type RunClient } from '../ports'
import { setup, send } from './test-support/chat-fixture'
const modes = ['default', 'plan', 'accept_edits', 'auto', 'full_access'] as const
describe('ChatPanel purpose controls', () => {
  it('hides Skills and permission controls in regular chat', async () => {
    setup(false, null, { props: { permissionModes: ['default', 'plan'], skills: [] } })
    expect(screen.queryByRole('button', { name: /^Permission mode:/ })).toBeNull()
    expect(screen.queryByLabelText('Skills for this message')).toBeNull()
  })

  it('shows Skills and permission controls for agent runs', async () => {
    setup(false, null, {
      skills: { list: () => Promise.resolve([]) },
      props: { purpose: 'agent', permissionModes: ['default', 'plan'], skills: [] },
    })
    expect(await screen.findByRole('button', { name: 'Permission mode: Ask each time' })).toBeTruthy()
    expect(await screen.findByLabelText('Skills for this message')).toBeTruthy()
  })
})

describe('ChatPanel permission modes', () => {
  async function choose(name: string) {
    const trigger = screen.getByRole('button', { name: /^Permission mode:/ })
    await waitFor(() => expect(trigger.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(trigger)
    await fireEvent.click(await screen.findByRole('menuitem', { name: new RegExp(`^${name}`) }))
  }
  it('offers only host modes, sends Plan and keeps approval cards', async () => {
    const { runs, approve } = setup(false, null, { props: { purpose: 'agent', permissionModes: ['default', 'plan'] } })
    await fireEvent.click(screen.getByRole('button', { name: 'Permission mode: Ask each time' }))
    expect(screen.queryByRole('menuitem', { name: /^Smart approval/ })).toBeNull()
    expect(screen.queryByRole('menuitem', { name: /^Full access/ })).toBeNull()
    await fireEvent.click(screen.getByRole('menuitem', { name: /^Plan only/ }))
    expect(screen.getByRole('status').textContent).toBe('Plan only')
    await send('inspect')
    expect(runs[0]?.request.permissionMode).toBe('plan')
    expect(
      screen.getByRole('button', { name: 'Permission mode: Plan only' }).hasAttribute('disabled'),
    ).toBe(true)
    runs[0]?.emit({ kind: 'tool_call', callId: 'c1', name: 'read', source: 'host' })
    runs[0]?.emit({ kind: 'approval_required', callId: 'c1', name: 'read', argsPreview: '{}' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Allow once' }))
    expect(approve).toHaveBeenCalledWith('r1', 'c1', true)
  })

  it('confirms FullAccess and consumes it for exactly one run, not regeneration', async () => {
    const { runs, notify } = setup(true, null, { props: { purpose: 'agent', permissionModes: [...modes] } })
    await choose('Full access')
    expect(notify.confirm).toHaveBeenCalledWith(expect.objectContaining({ danger: true }))
    await send('execute')
    expect(runs[0]?.request.permissionMode).toBe('full_access')
    expect(screen.getByRole('button', { name: 'Permission mode: Ask each time' })).toBeTruthy()
    runs[0]?.emit({ kind: 'text_delta', text: 'completed reply' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await send('continue normally')
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await choose('Full access')
    expect(notify.confirm).toHaveBeenCalledTimes(2)
    await send('explicit again')
    expect(runs[2]?.request.permissionMode).toBe('full_access')
  })

  it('does not accept FullAccess from a saved prop or a cancelled confirmation', async () => {
    const { runs, notify } = setup(false, null, {
      props: { purpose: 'agent', permissionModes: [...modes], permissionMode: 'full_access' },
    })
    expect(screen.getByRole('button', { name: 'Permission mode: Ask each time' })).toBeTruthy()
    notify.confirm.mockResolvedValueOnce(false)
    await choose('Full access')
    await send('read')
    expect(runs[0]?.request.permissionMode).toBe('default')
  })

  it('consumes FullAccess even if run acceptance fails', async () => {
    const run = vi.fn<RunClient['start']>(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    setup(false, null, { props: { purpose: 'agent', permissionModes: [...modes] }, chat: { start: run } })
    await choose('Full access')
    await send('execute')
    await waitFor(() => expect(run).toHaveBeenCalledTimes(1))
    expect(run.mock.calls[0]?.[0].permissionMode).toBe('full_access')
    await send('retry')
    expect(run.mock.calls[1]?.[0].permissionMode).toBe('default')
  })
})
