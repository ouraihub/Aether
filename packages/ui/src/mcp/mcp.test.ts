import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, KitClientError, type ServerDto, type ServerSave } from '../ports'
import Harness from './Harness.test.svelte'

const server = (over: Partial<ServerDto> = {}): ServerDto => ({
  id: 'm1',
  slug: 'fake',
  name: 'Fake',
  transport: { kind: 'http', url: 'http://127.0.0.1:1/mcp' },
  secretKeys: [],
  enabled: false,
  trusted: false,
  allowPrivate: false,
  tools: [],
  ...over,
})

describe('McpClientSettings', () => {
  it('adds a server with write-only credentials, lists tested tools at ask, confirms trust', async () => {
    let current = server()
    const save = vi.fn((input: ServerSave) => {
      current = { ...current, enabled: input.enabled, trusted: input.trusted }
      return Promise.resolve([current])
    })
    const tools = [{ name: 'echo', description: 'Echoes', readOnly: true, approval: 'ask' as const }]
    const refresh = vi.fn(() => {
      current = { ...current, tools }
      return Promise.resolve(tools)
    })
    const ports = createFakePorts({
      mcpClient: {
        list: () => Promise.resolve(current.trusted || current.tools.length ? [current] : []),
        save,
        refresh,
      },
    })
    const confirm = vi.fn(() => Promise.resolve(true))
    const notify = { toast: vi.fn(), confirm }
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
    render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } })

    await fireEvent.click(await screen.findByRole('button', { name: 'Add server' }))
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Fake' } })
    await fireEvent.input(screen.getByLabelText('Tool prefix'), { target: { value: 'fake' } })
    await fireEvent.input(screen.getByLabelText('URL'), { target: { value: 'http://127.0.0.1:1/mcp' } })
    await fireEvent.input(screen.getByLabelText(/Headers/), { target: { value: 'Authorization: Bearer x' } })
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1))
    expect(save.mock.calls[0]?.[0]).toEqual({
      slug: 'fake',
      name: 'Fake',
      transport: { kind: 'http', url: 'http://127.0.0.1:1/mcp' },
      enabled: false,
      trusted: false,
      allowPrivate: false,
      secret: { headers: { Authorization: 'Bearer x' } },
    })

    await fireEvent.click(await screen.findByRole('button', { name: 'Test connection' }))
    expect(await screen.findByText('echo')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Approval for echo' }).textContent).toContain('Ask')
    expect(screen.getByText(/read-only, can be set to auto/)).toBeTruthy()

    await fireEvent.click(screen.getByRole('switch', { name: 'Trusted' }))
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2))
    expect(confirm).toHaveBeenCalledTimes(1)
    // An update without touching credentials keeps them: no `secret` key at all.
    expect(save.mock.calls[1]?.[0]).toEqual({
      id: 'm1',
      slug: 'fake',
      name: 'Fake',
      transport: { kind: 'http', url: 'http://127.0.0.1:1/mcp' },
      enabled: false,
      trusted: true,
      allowPrivate: false,
    })
  })

  it('edits a stdio server: command line, env credentials, no private-network switch', async () => {
    const current = server({
      transport: { kind: 'stdio', command: 'npx', args: ['-y', 'srv'] },
      secretKeys: ['OLD_TOKEN'],
    })
    const save = vi.fn<(input: ServerSave) => Promise<ServerDto[]>>(() => Promise.resolve([current]))
    const ports = createFakePorts({ mcpClient: { list: () => Promise.resolve([current]), save } })
    const notify = { toast: vi.fn(), confirm: vi.fn(() => Promise.resolve(true)) }
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
    render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } })

    expect(await screen.findByText('Local command (stdio) · npx -y srv')).toBeTruthy()
    expect(screen.queryByRole('switch', { name: 'Allow private network' })).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    expect(screen.getByLabelText('Command')).toHaveProperty('value', 'npx')
    expect(screen.queryByLabelText('URL')).toBeNull()
    await fireEvent.input(screen.getByLabelText(/Arguments/), { target: { value: '-y\n\nsrv\n--port=1' } })
    await fireEvent.input(screen.getByLabelText('Working directory'), { target: { value: 'C:\\work' } })
    await fireEvent.click(screen.getByRole('switch', { name: /Replace credentials/ }))
    expect(screen.queryByLabelText(/Headers/)).toBeNull()
    await fireEvent.input(screen.getByLabelText(/Environment variables/), {
      target: { value: 'GITHUB_TOKEN=abc' },
    })
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1))
    expect(save.mock.calls[0]?.[0]).toEqual({
      id: 'm1',
      slug: 'fake',
      name: 'Fake',
      transport: { kind: 'stdio', command: 'npx', args: ['-y', 'srv', '--port=1'], cwd: 'C:\\work' },
      enabled: false,
      trusted: false,
      allowPrivate: false,
      secret: { env: { GITHUB_TOKEN: 'abc' } },
    })
  })
})

function mountClient(
  save: (input: ServerSave) => Promise<ServerDto[]>,
  records = [server({ secretKeys: ['Authorization'] })],
) {
  const notify = { toast: vi.fn(), confirm: vi.fn(async () => true) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const ports = createFakePorts({ mcpClient: { list: async () => records, save } })
  return { ...render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger } }), notify }
}

async function openEditor() {
  await fireEvent.click(await screen.findByRole('button', { name: 'Edit' }))
  return within(screen.getByRole('dialog'))
}

describe('MCP write-only editor sessions', () => {
  it('preserves existing credentials unless replacement is explicitly selected; empty replacement clears', async () => {
    const save = vi.fn<(input: ServerSave) => Promise<ServerDto[]>>(async () => [
      server({ secretKeys: ['Authorization'] }),
    ])
    mountClient(save)
    let dialog = await openEditor()
    expect(dialog.queryByLabelText(/Headers/)).toBeNull()
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(save.mock.calls[0]?.[0]).not.toHaveProperty('secret')
    dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(save.mock.calls[1]?.[0].secret).toEqual({})
  })

  it('requires explicit credential replacement on transport change and never mixes HTTP secrets into env', async () => {
    const save = vi.fn<(input: ServerSave) => Promise<ServerDto[]>>(async () => [server()])
    const { notify } = mountClient(save)
    const dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: old-input' } })
    await fireEvent.keyDown(dialog.getByRole('button', { name: 'Connection' }), { key: 'ArrowDown' })
    await fireEvent.pointerUp(await screen.findByRole('option', { name: 'Local command (stdio)' }))
    await fireEvent.input(dialog.getByLabelText('Command'), { target: { value: ' npx ' } })
    await fireEvent.input(dialog.getByLabelText(/Arguments/), {
      target: { value: '  keep spaces  \n\n$literal;`not-shell`' },
    })
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    expect(save).not.toHaveBeenCalled()
    expect(notify.toast).toHaveBeenCalledWith('error', expect.any(String))
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    expect(dialog.getByLabelText(/Environment variables/)).toHaveProperty('value', '')
    await fireEvent.input(dialog.getByLabelText(/Environment variables/), { target: { value: 'TOKEN=new=value' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1))
    expect(save.mock.calls[0]?.[0].transport).toEqual({
      kind: 'stdio',
      command: 'npx',
      args: ['  keep spaces  ', '$literal;`not-shell`'],
    })
    expect(save.mock.calls[0]?.[0].secret).toEqual({ env: { TOKEN: 'new=value' } })
  })

  for (const fail of [false, true]) {
    it(`ignores old save ${fail ? 'failure' : 'success'} after closing and reopening the same target`, async () => {
      const saving = Promise.withResolvers<ServerDto[]>()
      const save = vi.fn(() => saving.promise)
      const { notify } = mountClient(save)
      let dialog = await openEditor()
      await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
      await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: old-secret' } })
      await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
      await fireEvent.click(dialog.getByRole('button', { name: 'Cancel' }))
      dialog = await openEditor()
      await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
      expect(dialog.getByLabelText(/Headers/)).toHaveProperty('value', '')
      await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: new-secret' } })
      if (fail) saving.reject(new KitClientError({ code: 'internal', message: 'old failure', details: {} }))
      else saving.resolve([server({ name: 'Old response' })])
      await saving.promise.catch(() => {})
      await waitFor(() => expect(dialog.getByLabelText(/Headers/)).toHaveProperty('value', 'Authorization: new-secret'))
      expect(screen.getByRole('dialog')).toBeTruthy()
      expect(screen.queryByText('Old response')).toBeNull()
      expect(notify.toast).not.toHaveBeenCalled()
    })
  }

  it('retains a current failed secret for retry but clears it when dismissed', async () => {
    const save = vi.fn<(input: ServerSave) => Promise<ServerDto[]>>(async () => {
      throw new KitClientError({ code: 'internal', message: 'failed', details: {} })
    })
    const { notify } = mountClient(save)
    let dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: retry-secret' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(notify.toast).toHaveBeenCalled())
    expect(dialog.getByLabelText(/Headers/)).toHaveProperty('value', 'Authorization: retry-secret')
    await fireEvent.click(dialog.getByRole('button', { name: 'Close' }))
    dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    expect(dialog.getByLabelText(/Headers/)).toHaveProperty('value', '')
  })

  it('preserves a newer same-text secret revision while an earlier version saves', async () => {
    const saving = Promise.withResolvers<ServerDto[]>()
    mountClient(() => saving.promise)
    const dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    const input = dialog.getByLabelText(/Headers/)
    await fireEvent.input(input, { target: { value: 'Authorization: same' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await fireEvent.input(input, { target: { value: 'Authorization: changed' } })
    await fireEvent.input(input, { target: { value: 'Authorization: same' } })
    saving.resolve([server()])
    await saving.promise
    await waitFor(() => expect(dialog.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', false))
    expect(input).toHaveProperty('value', 'Authorization: same')
  })

  it('clears successfully saved secrets without discarding newer nonsecret edits', async () => {
    const saving = Promise.withResolvers<ServerDto[]>()
    mountClient(() => saving.promise)
    const dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: saved-secret' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await fireEvent.input(dialog.getByLabelText('Name'), { target: { value: 'New local name' } })
    saving.resolve([server({ secretKeys: ['Authorization'] })])
    await saving.promise
    await waitFor(() => expect(dialog.queryByLabelText(/Headers/)).toBeNull())
    expect(dialog.getByLabelText('Name')).toHaveProperty('value', 'New local name')
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    expect(dialog.getByLabelText(/Headers/)).toHaveProperty('value', '')
  })

  it('does not update a different target or expose old secrets when its save resolves', async () => {
    const saving = Promise.withResolvers<ServerDto[]>()
    mountClient(() => saving.promise, [server(), server({ id: 'm2', name: 'Other', slug: 'other' })])
    await screen.findByRole('listitem', { name: 'Fake' })
    await fireEvent.click(within(screen.getByRole('listitem', { name: 'Fake' })).getByRole('button', { name: 'Edit' }))
    let dialog = within(screen.getByRole('dialog'))
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: old-secret' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Cancel' }))
    await fireEvent.click(within(screen.getByRole('listitem', { name: 'Other' })).getByRole('button', { name: 'Edit' }))
    dialog = within(screen.getByRole('dialog'))
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    expect(dialog.getByLabelText(/Headers/)).toHaveProperty('value', '')
    saving.resolve([server({ name: 'Stale' })])
    await saving.promise
    expect(dialog.getByLabelText('Name')).toHaveProperty('value', 'Other')
    expect(screen.queryByText('Stale')).toBeNull()
  })

  it('does not notify or reopen an editor after unmounting a pending save', async () => {
    const saving = Promise.withResolvers<ServerDto[]>()
    const h = mountClient(() => saving.promise)
    const dialog = await openEditor()
    await fireEvent.click(dialog.getByRole('switch', { name: /Replace credentials/ }))
    await fireEvent.input(dialog.getByLabelText(/Headers/), { target: { value: 'Authorization: old-secret' } })
    await fireEvent.click(dialog.getByRole('button', { name: 'Save' }))
    h.unmount()
    saving.reject(new Error('late save failure'))
    await saving.promise.catch(() => {})
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(h.notify.toast).not.toHaveBeenCalled()
  })
})
