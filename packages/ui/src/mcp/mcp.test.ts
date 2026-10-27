import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, type ServerDto, type ServerSave } from '../ports'
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
