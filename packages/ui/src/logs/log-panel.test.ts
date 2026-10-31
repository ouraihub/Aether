import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'
import { afterEach, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts } from '../ports'
import type { KitContext } from '../context'
import Harness from './Harness.test.svelte'

afterEach(cleanup)

it(
  'reads logs without desktop capability and hides the directory action',
  async () => {
  const ports = createFakePorts({
    omit: ['desktop'],
    logs: {
      tail: () =>
        Promise.resolve([
          {
            seq: 1,
            time: 'now',
            level: 'INFO',
            target: 'ui',
            message: 'Readable log',
          },
        ]),
    },
  })
  const kit: KitContext = {
    ports,
    i18n: createI18n('en'),
    notify: { toast: vi.fn(), confirm: () => Promise.resolve(false) },
    logger: { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() },
  }
  render(Harness, { kit })
  await screen.findByText(/Readable log/)
  expect(screen.queryByRole('button', { name: 'Open folder' })).toBeNull()
})

it(
  'opens the log directory only through the optional desktop capability',
  async () => {
  const openLogsDirectory = vi.fn(() => Promise.resolve())
  const kit: KitContext = {
    ports: createFakePorts({ desktop: { openLogsDirectory } }),
    i18n: createI18n('en'),
    notify: { toast: vi.fn(), confirm: () => Promise.resolve(false) },
    logger: { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() },
  }
  render(Harness, { kit })
  await fireEvent.click(screen.getByRole('button', { name: 'Open folder' }))
  expect(openLogsDirectory).toHaveBeenCalledOnce()
})
