import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  KitClientError,
  type FakePortsOptions,
  type Preview,
  type RunRequest,
  type SkillMeta,
  type Source,
} from '../ports'
import Harness from './Harness.test.svelte'

const meta = (revision: number): SkillMeta => ({
  id: 's1',
  name: 'writer',
  description: 'Writes',
  version: null,
  origin: 'imported',
  revision,
  bodyChars: 4,
  updatedAt: 0,
})

const preview = (body: string): Preview => ({
  digest: `d-${body}`,
  name: 'writer',
  description: 'Writes',
  version: '1.0.0',
  body,
  license: null,
  extra: {},
  action: 'install',
  current: null,
})

function mount(options: FakePortsOptions, skills?: { id: string; revision: number }[]) {
  const ports = createFakePorts({
    providers: {
      modelOptions: () =>
        Promise.resolve([{ endpointId: 'e1', modelId: 'm1', label: 'm1', modelType: 'chat', supportsTools: null }]),
    },
    ...options,
  })
  const notify = { toast: vi.fn(), confirm: () => Promise.resolve(true) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  render(Harness, { kit: { ports, i18n: createI18n('en'), notify, logger }, ...(skills ? { skills } : {}) })
  return notify
}

describe('SkillsSettings', () => {
  it('previews the full text, re-confirms after content_changed, installs with the shown digest', async () => {
    const install = vi.fn()
    install.mockRejectedValueOnce(
      new KitClientError({ code: 'conflict', message: 'changed', details: { reason: 'content_changed' } }),
    )
    install.mockResolvedValueOnce([meta(1)])
    const previews = [preview('old body'), preview('new body')]
    const previewFn = vi.fn<(source: Source) => Promise<Preview>>(() =>
      Promise.resolve(previews.shift() ?? preview('none')),
    )
    mount({ skills: { preview: previewFn, install } })

    const file = new File(['---\nname: writer\n---\nold body'], 'SKILL.md')
    await fireEvent.change(screen.getByLabelText('Import SKILL.md or .zip'), { target: { files: [file] } })
    await screen.findByText('old body')
    const source = previewFn.mock.calls[0]?.[0]
    expect(source).toMatchObject({ bytes: { fileName: 'SKILL.md' } })

    await fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    await screen.findByText('new body')
    expect(install).toHaveBeenNthCalledWith(1, source, 'd-old body', false)

    await fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    await screen.findByText('writer')
    expect(install).toHaveBeenNthCalledWith(2, source, 'd-new body', false)
  })
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
      return Promise.resolve({ runId: 'r2', conversationId: null })
    })
    const get = vi.fn(() => Promise.resolve({ ...meta(2), body: 'edited body', license: null, extra: {} }))
    mount({ chat: { run }, skills: { list: () => Promise.resolve([meta(2)]), get } }, [{ id: 's1', revision: 1 }])

    const send = async (text: string) => {
      await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), { target: { value: text } })
      await fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    }
    await send('one')
    expect(requests[0]?.skills).toEqual([{ id: 's1', revision: 1 }])
    await screen.findByText('Skill updated, confirm again')
    await screen.findByText('edited body')
    await fireEvent.click(screen.getByRole('button', { name: 'Use this version' }))

    await send('two')
    await waitFor(() => expect(requests).toHaveLength(2))
    expect(requests[1]?.skills).toEqual([{ id: 's1', revision: 2 }])
  })
})
