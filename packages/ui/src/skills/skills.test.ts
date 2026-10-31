import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  KitClientError,
  type FakePortsOptions,
  type Preview,
  type SkillMeta,
  type Source,
} from '../ports'
import Harness from './Harness.test.svelte'

const meta = (
  revision: number,
  files: Pick<SkillMeta, 'hasScripts' | 'fileCount'> = {
    hasScripts: false,
    fileCount: 0,
  },
): SkillMeta => ({
  id: 's1',
  name: 'writer',
  description: 'Writes',
  version: null,
  origin: 'imported',
  revision,
  bodyChars: 4,
  updatedAt: 0,
  ...files,
})

const preview = (body: string): Preview => ({
  digest: `d-${body}`,
  name: 'writer',
  description: 'Writes',
  version: '1.0.0',
  body,
  license: null,
  extra: {},
  files: [],
  action: 'install',
  current: null,
})

function mount(
  options: FakePortsOptions,
  skills?: { id: string; revision: number }[],
) {
  const ports = createFakePorts({
    providers: {
      modelOptions: () =>
        Promise.resolve([
          {
            endpointId: 'e1',
            modelId: 'm1',
            label: 'm1',
            name: null,
            modelType: 'chat',
            supportsTools: null,
            reasoning: null,
            nativeWebSearch: false,
            canVision: false,
          },
        ]),
    },
    ...options,
  })
  const notify = { toast: vi.fn(), confirm: () => Promise.resolve(true) }
  const logger = {
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }
  const view = render(Harness, {
    kit: { ports, i18n: createI18n('en'), notify, logger },
    ...(skills ? { skills } : {}),
  })
  return { ...notify, unmount: view.unmount }
}

async function addFrom(name: string) {
  await fireEvent.click(screen.getByRole('button', { name: 'Add skill' }))
  await fireEvent.click(await screen.findByRole('menuitem', { name }))
}


describe('SkillsSettings', () => {
  it(
    'previews the full text, re-confirms after content_changed, installs with the ' +
    'shown digest',
    async () => {
    const install = vi.fn()
    install.mockRejectedValueOnce(
      new KitClientError({
        code: 'conflict',
        message: 'changed',
        details: { reason: 'content_changed' },
      }),
    )
    install.mockResolvedValueOnce([meta(1)])
    const previews = [preview('old body'), preview('new body')]
    const previewFn = vi.fn<(source: Source) => Promise<Preview>>(() =>
      Promise.resolve(previews.shift() ?? preview('none')),
    )
    mount({ skills: { preview: previewFn, install } })

    const file = new File(['---\nname: writer\n---\nold body'], 'SKILL.md')
    await fireEvent.change(screen.getByLabelText('Import SKILL.md or .zip'), {
      target: { files: [file] },
    })
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

  it(
    'lists packaged files and warns about scripts before install; the list marks ' +
    'skills with scripts',
    async () => {
    const scripted: Preview = {
      ...preview('run it'),
      files: [
        { path: 'references/guide.md', kind: 'reference', size: 12 },
        { path: 'scripts/run.py', kind: 'script', size: 9 },
      ],
    }
    const install = vi.fn(() =>
      Promise.resolve([meta(1, { hasScripts: true, fileCount: 2 })]),
    )
    mount({ skills: { preview: () => Promise.resolve(scripted), install } })

    const file = new File(['zip'], 'tool.zip')
    await fireEvent.change(screen.getByLabelText('Import SKILL.md or .zip'), {
      target: { files: [file] },
    })
    await screen.findByText('scripts/run.py')
    expect(screen.getByText('references/guide.md')).toBeTruthy()
    expect(screen.getByRole('alert').textContent).toContain(
      'never runs them automatically',
    )

    await fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    await screen.findByText('Has scripts')
    expect(screen.getByText(/2 files/)).toBeTruthy()
  })

  it('shows no script notice for a skill with only references', async () => {
    const plain: Preview = {
      ...preview('read it'),
      files: [{ path: 'references/a.md', kind: 'reference', size: 1 }],
    }
    mount({ skills: { preview: () => Promise.resolve(plain) } })
    await fireEvent.change(screen.getByLabelText('Import SKILL.md or .zip'), {
      target: { files: [new File(['zip'], 'a.zip')] },
    })
    await screen.findByText('references/a.md')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it(
    'prepares an online result without installing until confirmation; closing never ' +
    'installs',
    async () => {
    const source: Source = {
      bytes: { fileName: 'writer.zip', bytes: [1, 2, 3] },
    }
    const search = vi.fn(async () => [
      {
        name: 'writer',
        description: 'Writes',
        url: 'https://github.com/team/skills/tree/main/writer',
        origin: 'GitHub',
      },
    ])
    const prepare = vi.fn(async () => ({ source, origin: 'GitHub' }))
    const previewFn = vi.fn(async () => preview('online skill body'))
    const install = vi.fn(async () => [meta(1)])
    mount({
      skillSources: { search, prepare },
      skills: { preview: previewFn, install },
    })
    await addFrom('Search online')
    await fireEvent.input(screen.getByLabelText('Keywords or GitHub URL'), {
      target: { value: 'https://github.com/team/skills' },
    })
    await fireEvent.click(screen.getByRole('button', { name: /^Search$/ }))
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Preview skill' }),
    )
    await screen.findByText('online skill body')
    expect(search).toHaveBeenCalledWith('https://github.com/team/skills', {
      signal: expect.any(AbortSignal),
    })
    expect(prepare).toHaveBeenCalledWith(
      'https://github.com/team/skills/tree/main/writer',
      {
        signal: expect.any(AbortSignal),
      },
    )
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    await screen.findByLabelText('Keywords or GitHub URL')
    expect(install).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Preview skill' }))
    await screen.findByText('online skill body')
    await fireEvent.click(screen.getByRole('button', { name: /^Install$/ }))
    await waitFor(() =>
      expect(install).toHaveBeenCalledWith(
        source,
        'd-online skill body',
        false,
      ),
    )
  })

  it(
    'searches a user-selected system folder and previews only the selected ' +
    'authorized candidate',
    async () => {
    const candidate = {
      name: 'project-writer',
      path: 'E:/workspace/project/.claude/skills/writer',
      origin: 'Selected folder',
    }
    const source: Source = { bytes: { fileName: 'writer.zip', bytes: [7] } }
    const discover = vi.fn(async () => [])
    const pickSearchDirectory = vi.fn(async () => [candidate])
    const prepareSystem = vi.fn(async () => ({
      source,
      origin: candidate.path,
    }))
    const previewFn = vi.fn(async () => preview('system skill body'))
    const install = vi.fn()
    mount({
      skillSources: { discover, pickSearchDirectory, prepareSystem },
      skills: { preview: previewFn, install },
    })
    await addFrom('Search this computer')
    await waitFor(() => expect(discover).toHaveBeenCalledTimes(1))
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Choose skill folder' }),
    )
    await screen.findByText(/E:\/workspace\/project/)
    expect(prepareSystem).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Preview skill' }))
    await screen.findByText('system skill body')
    expect(prepareSystem).toHaveBeenCalledWith(candidate.path, {
      signal: expect.any(AbortSignal),
    })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
  })


  it(
    'imports a selected local folder through the same preview boundary without ' +
    'installing',
    async () => {
    const source: Source = { bytes: { fileName: 'folder.zip', bytes: [4, 5] } }
    const pickDirectory = vi.fn(async () => ({
      source,
      origin: 'E:/skills/writer',
    }))
    const previewFn = vi.fn(async () => preview('local folder body'))
    const install = vi.fn()
    mount({
      skillSources: { pickDirectory },
      skills: { preview: previewFn, install },
    })
    await addFrom('Import local file or folder')
    await fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }))
    await screen.findByText('local folder body')
    expect(pickDirectory).toHaveBeenCalledWith({
      signal: expect.any(AbortSignal),
    })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
  })

  it(
    'ignores preparation completed after its source dialog was closed',
    async () => {
    const pending = Promise.withResolvers<{ source: Source; origin: string }>()
    const pickDirectory = vi.fn(() => pending.promise)
    const previewFn = vi.fn()
    const install = vi.fn()
    mount({
      skillSources: { pickDirectory },
      skills: { preview: previewFn, install },
    })
    await addFrom('Import local file or folder')
    await fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }))
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    pending.resolve({
      source: { bytes: { fileName: 'late.zip', bytes: [9] } },
      origin: 'late folder',
    })
    await pending.promise
    await addFrom('Import local file or folder')
    expect(previewFn).not.toHaveBeenCalled()
    expect(install).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: /^Install$/ })).toBeNull()
  })

})
