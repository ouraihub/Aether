import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  type IssuedToken,
  type McpServerPort,
  type Status,
} from '../ports'
import { buildClaudePosixCommand, buildMcpJsonConfig } from './config'
import Harness from './Harness.test.svelte'

const status: Status = {
  running: true,
  port: 3849,
  url: 'http://127.0.0.1:3849/mcp',
  toolCount: 1,
}
const issued = (name = 'Example', token = 'example-token'): IssuedToken => ({
  id: name,
  name,
  token,
  scopes: ['read'],
  createdAt: 0,
  lastUsedAt: null,
  revokedAt: null,
})

function mount(over: Partial<McpServerPort> = {}, serverName = 'aether') {
  const notify = { toast: vi.fn(), confirm: vi.fn(async () => true) }
  const logger = {
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }
  const ports = createFakePorts({
    mcpServer: {
      status: async () => status,
      scopes: async () => [
        {
          id: 'read',
          labelKey: 'read-scope',
          destructive: false,
          enabled: true,
        },
      ],
      tokens: async () => [],
      issueToken: async () => issued(),
      ...over,
    },
  })
  return {
    ...render(Harness, {
      mode: 'server',
      serverName,
      kit: { ports, i18n: createI18n('en'), notify, logger },
    }),
    notify,
  }
}

async function grantAndIssue(name = 'Example') {
  await screen.findByRole('button', { name: 'Issue token' })
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Stop' })).toHaveProperty(
      'disabled',
      false,
    ),
  )
  await fireEvent.input(screen.getByLabelText('Name'), {
    target: { value: name },
  })
  const grants = screen.getAllByRole('switch', { name: 'read-scope (read)' })
  const grant = grants[1]
  if (!grant) throw new Error('Expected token scope switch')
  await fireEvent.click(grant)
  await fireEvent.click(screen.getByRole('button', { name: 'Issue token' }))
}

describe('MCP server configuration and token display', () => {
  it(
    'copies the actual issued token and URL through pure helpers with ' +
      'explicit POSIX labeling',
    async () => {
      const writeText = vi.fn<(text: string) => Promise<void>>(() =>
        Promise.resolve(),
      )
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: { writeText },
      })
      mount()
      await grantAndIssue()
      await screen.findByText('example-token')
      const input = {
        serverName: 'aether',
        url: status.url,
        token: 'example-token',
      }
      await fireEvent.click(
        screen.getByRole('button', {
          name: 'Copy Claude Code command (POSIX shell)',
        }),
      )
      await waitFor(() =>
        expect(writeText).toHaveBeenCalledWith(buildClaudePosixCommand(input)),
      )
      await fireEvent.click(
        screen.getByRole('button', { name: 'Copy JSON config' }),
      )
      await waitFor(() =>
        expect(writeText).toHaveBeenCalledWith(buildMcpJsonConfig(input)),
      )
      await fireEvent.click(screen.getByRole('button', { name: 'Done' }))
      expect(screen.queryByText('example-token')).toBeNull()
      expect(
        screen.queryByRole('button', { name: 'Copy JSON config' }),
      ).toBeNull()
    },
  )

  it.each([
    { serverName: 'aether', url: '', token: 'example-token' },
    { serverName: '', url: status.url, token: 'example-token' },
    { serverName: 'aether', url: status.url, token: '' },
  ])(
    'disables configuration actions without actual inputs: %o',
    async ({ serverName, url, token }) => {
      mount(
        {
          status: async () => ({ ...status, url }),
          issueToken: async () => issued('Example', token),
        },
        serverName,
      )
      await grantAndIssue()
      const json = await screen.findByRole('button', {
        name: 'Copy JSON config',
      })
      expect(json).toHaveProperty('disabled', true)
      expect(
        screen.getByRole('button', {
          name: 'Copy Claude Code command (POSIX shell)',
        }),
      ).toHaveProperty('disabled', true)
    },
  )

  it(
    'does not present an invalid Claude CLI name as a runnable POSIX command',
    async () => {
      mount({}, 'not a cli name')
      await grantAndIssue()
      expect(
        await screen.findByRole('button', {
          name: 'Copy Claude Code command (POSIX shell)',
        }),
      ).toHaveProperty('disabled', true)
      expect(
        screen.getByRole('button', { name: 'Copy JSON config' }),
      ).toHaveProperty('disabled', false)
    },
  )

  for (const fail of [false, true]) {
    it(
      `ignores late issuance ${fail ? 'failure' : 'success'} after changing ` +
        'target',
      async () => {
        const pending = Promise.withResolvers<IssuedToken>()
        const issueToken = vi
          .fn()
          .mockReturnValueOnce(pending.promise)
          .mockResolvedValueOnce(issued('New', 'new-token'))
        const h = mount({ issueToken })
        await grantAndIssue('Old')
        await h.rerender({ serverName: 'other-server' })
        await grantAndIssue('New')
        await screen.findByText('new-token')
        if (fail) pending.reject(new Error('late failure'))
        else pending.resolve(issued('Old', 'old-token'))
        await pending.promise.catch(() => {})
        expect(screen.queryByText('old-token')).toBeNull()
        expect(screen.getByText('new-token')).toBeTruthy()
        expect(h.notify.toast).not.toHaveBeenCalled()
      },
    )
  }

  for (const fail of [false, true]) {
    it(
      `suppresses late clipboard ${fail ? 'failure' : 'success'} after ` +
        'dismissing the token',
      async () => {
        const copying = Promise.withResolvers<void>()
        const writeText = vi.fn(() => copying.promise)
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value: { writeText },
        })
        const h = mount()
        await grantAndIssue()
        await screen.findByText('example-token')
        await fireEvent.click(
          screen.getByRole('button', { name: 'Copy JSON config' }),
        )
        expect(writeText).toHaveBeenCalledTimes(1)
        await fireEvent.click(screen.getByRole('button', { name: 'Done' }))
        if (fail) copying.reject(new Error('late clipboard failure'))
        else copying.resolve()
        await copying.promise.catch(() => {})
        expect(screen.queryByText('example-token')).toBeNull()
        expect(h.notify.toast).not.toHaveBeenCalled()
      },
    )
  }

  it(
    'clears an issued token on target changes and does not resurrect it ' +
      'on return',
    async () => {
      const h = mount()
      await grantAndIssue()
      await screen.findByText('example-token')
      await h.rerender({ serverName: 'other-server' })
      expect(screen.queryByText('example-token')).toBeNull()
      await h.rerender({ serverName: 'aether' })
      expect(screen.queryByText('example-token')).toBeNull()
    },
  )

  it(
    'replaces the issued display without accepting a previous copy completion',
    async () => {
      const copying = Promise.withResolvers<void>()
      const writeText = vi.fn(() => copying.promise)
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: { writeText },
      })
      const issueToken = vi
        .fn()
        .mockResolvedValueOnce(issued())
        .mockResolvedValueOnce(issued('Next', 'next-token'))
      const h = mount({ issueToken })
      await grantAndIssue()
      await screen.findByText('example-token')
      await fireEvent.click(
        screen.getByRole('button', { name: 'Copy JSON config' }),
      )
      await grantAndIssue('Next')
      await screen.findByText('next-token')
      copying.resolve()
      await copying.promise
      expect(screen.queryByText('example-token')).toBeNull()
      expect(screen.getByText('next-token')).toBeTruthy()
      expect(h.notify.toast).not.toHaveBeenCalled()
    },
  )

  it(
    'keeps destructive-scope confirmation before changing enabled scopes',
    async () => {
      const setScopes = vi.fn(async () => [
        {
          id: 'write',
          labelKey: 'write-scope',
          destructive: true,
          enabled: true,
        },
      ])
      const h = mount({
        scopes: async () => [
          {
            id: 'write',
            labelKey: 'write-scope',
            destructive: true,
            enabled: false,
          },
        ],
        setScopes,
      })
      const switches = await screen.findAllByRole('switch', {
        name: 'write-scope (write)',
      })
      const scope = switches[0]
      if (!scope) throw new Error('Expected server scope switch')
      await waitFor(() => expect(scope).toHaveProperty('disabled', false))
      h.notify.confirm.mockResolvedValueOnce(false)
      await fireEvent.click(scope)
      await waitFor(() =>
        expect(scope.getAttribute('aria-checked')).toBe('false'),
      )
      expect(setScopes).not.toHaveBeenCalled()
      await fireEvent.click(scope)
      await waitFor(() => expect(setScopes).toHaveBeenCalledWith(['write']))
      expect(h.notify.confirm).toHaveBeenCalledWith(
        expect.objectContaining({ danger: true }),
      )
    },
  )

  it.each(['not a URL', 'file:///tmp/mcp', '   '])(
    'does not offer config actions for an invalid HTTP URL: %s',
    async (url) => {
      mount({ status: async () => ({ ...status, url }) })
      await grantAndIssue()
      expect(
        await screen.findByRole('button', { name: 'Copy JSON config' }),
      ).toHaveProperty('disabled', true)
      expect(
        screen.getByRole('button', {
          name: 'Copy Claude Code command (POSIX shell)',
        }),
      ).toHaveProperty('disabled', true)
    },
  )

  it(
    'does not display or notify a token response after unmount',
    async () => {
      const pending = Promise.withResolvers<IssuedToken>()
      const h = mount({ issueToken: () => pending.promise })
      await grantAndIssue()
      h.unmount()
      pending.resolve(issued('Unmounted', 'late-token'))
      await pending.promise
      expect(screen.queryByText('late-token')).toBeNull()
      expect(h.notify.toast).not.toHaveBeenCalled()
    },
  )
})
