import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import { createFakePorts, KitClientError, type RunRequest } from '../ports'
import Harness from './Harness.test.svelte'

const meta = (revision: number) => ({
  id: 's1',
  name: 'writer',
  description: 'Writes',
  version: null,
  origin: 'imported' as const,
  revision,
  bodyChars: 4,
  updatedAt: 0,
  hasScripts: false,
  fileCount: 0,
})

describe('SkillPicker in ChatPanel', () => {
  it('sends the selection; skill_changed opens the new text and re-pins on confirm', async () => {
    const requests: RunRequest[] = []
    const run = vi.fn((request: RunRequest) => {
      requests.push(request)
      if (requests.length === 1) {
        return Promise.reject(
          new KitClientError({
            code: 'conflict',
            message: 'changed',
            details: { reason: 'skill_changed', entity: 's1' },
          }),
        )
      }
      return Promise.resolve({
        runId: 'r2',
        conversationId: null,
        turnId: null,
      })
    })
    const get = vi.fn(() =>
      Promise.resolve({
        ...meta(2),
        body: 'edited body',
        license: null,
        extra: {},
        files: [],
      }),
    )
    const notify = { toast: vi.fn(), confirm: () => Promise.resolve(true) }
    const logger = {
      debug: vi.fn(),
      info: vi.fn(),
      warn: vi.fn(),
      error: vi.fn(),
    }
    const ports = createFakePorts({
      chat: { start: run },
      skills: { list: () => Promise.resolve([meta(2)]), get },
    })
    render(Harness, {
      kit: { ports, i18n: createI18n('en'), notify, logger },
      skills: [{ id: 's1', revision: 1 }],
    })

    const send = async (text: string) => {
      await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
        target: { value: text },
      })
      await fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    }
    await send('one')
    expect(requests[0]?.skills).toEqual([{ id: 's1', revision: 1 }])
    await screen.findByText('Skill updated, confirm again')
    await screen.findByText('edited body')
    await fireEvent.click(
      screen.getByRole('button', { name: 'Use this version' }),
    )

    await send('two')
    await waitFor(() => expect(requests).toHaveLength(2))
    expect(requests[1]?.skills).toEqual([{ id: 's1', revision: 2 }])
  })
})
