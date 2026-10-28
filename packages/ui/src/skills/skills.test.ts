import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from '../i18n'
import {
  createFakePorts,
  KitClientError,
  type FakePortsOptions,
  type Preview,
  type RunRequest,
  type RunEvent,
  type SkillAuthoringPort,
  type SkillDto,
  type SkillMeta,
  type Source,
} from '../ports'
import Harness from './Harness.test.svelte'

const meta = (
  revision: number,
  files: Pick<SkillMeta, 'hasScripts' | 'fileCount'> = { hasScripts: false, fileCount: 0 },
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

function mount(options: FakePortsOptions, skills?: { id: string; revision: number }[]) {
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
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
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

async function selectAuthoringModel() {
  await fireEvent.click(await screen.findByLabelText('Select a model'))
  await fireEvent.click(await screen.findByRole('option', { name: /m1/ }))
}

const creator: SkillDto = {
  ...meta(1),
  id: 'builtin-creator',
  name: 'skill-creator',
  origin: 'builtin',
  body: 'Complete creator instructions: capture intent, draft, evaluate, improve, package.',
  license: null,
  extra: {},
  files: [{ path: 'scripts/quick_validate.py', kind: 'script', size: 10 }],
}

function authoring(overrides: Partial<SkillAuthoringPort> = {}): SkillAuthoringPort {
  return {
    create: vi.fn(async () => 'authoring-1'),
    send: vi.fn<SkillAuthoringPort['send']>(async (_session, _input, _model, emit) => {
      emit({ kind: 'end', status: 'completed' })
      return { runId: 'authoring-run', conversationId: null, turnId: null }
    }),
    approve: vi.fn(async () => {}),
    cancel: vi.fn(async () => {}),
    prepare: vi.fn(async () => ({
      source: { bytes: { fileName: 'writer.zip', bytes: [1] } },
      origin: 'Authoring workspace',
    })),
    close: vi.fn(async () => {}),
    ...overrides,
  }
}

function mountAuthoring(api: SkillAuthoringPort, options: FakePortsOptions = {}) {
  return mount({
    ...options,
    skillSources: { ...options.skillSources, authoring: api },
    skills: {
      list: async () => [creator],
      get: async () => creator,
      ...options.skills,
    },
  })
}

async function startAuthoring() {
  await addFrom('Create with Agent')
  await fireEvent.click(await screen.findByRole('button', { name: 'Review skill-creator' }))
  await screen.findByText(creator.body)
  await fireEvent.click(screen.getByRole('button', { name: 'Confirm skill-creator and start' }))
  await screen.findByLabelText('Message to the skill creation assistant')
}

async function authorMessage(text: string) {
  await fireEvent.input(screen.getByLabelText('Message to the skill creation assistant'), { target: { value: text } })
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }))
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

  it('lists packaged files and warns about scripts before install; the list marks skills with scripts', async () => {
    const scripted: Preview = {
      ...preview('run it'),
      files: [
        { path: 'references/guide.md', kind: 'reference', size: 12 },
        { path: 'scripts/run.py', kind: 'script', size: 9 },
      ],
    }
    const install = vi.fn(() => Promise.resolve([meta(1, { hasScripts: true, fileCount: 2 })]))
    mount({ skills: { preview: () => Promise.resolve(scripted), install } })

    const file = new File(['zip'], 'tool.zip')
    await fireEvent.change(screen.getByLabelText('Import SKILL.md or .zip'), { target: { files: [file] } })
    await screen.findByText('scripts/run.py')
    expect(screen.getByText('references/guide.md')).toBeTruthy()
    expect(screen.getByRole('alert').textContent).toContain('never runs them automatically')

    await fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    await screen.findByText('Has scripts')
    expect(screen.getByText(/2 files/)).toBeTruthy()
  })

  it('shows no script notice for a skill with only references', async () => {
    const plain: Preview = { ...preview('read it'), files: [{ path: 'references/a.md', kind: 'reference', size: 1 }] }
    mount({ skills: { preview: () => Promise.resolve(plain) } })
    await fireEvent.change(screen.getByLabelText('Import SKILL.md or .zip'), {
      target: { files: [new File(['zip'], 'a.zip')] },
    })
    await screen.findByText('references/a.md')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('prepares an online result without installing until confirmation; closing never installs', async () => {
    const source: Source = { bytes: { fileName: 'writer.zip', bytes: [1, 2, 3] } }
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
    mount({ skillSources: { search, prepare }, skills: { preview: previewFn, install } })
    await addFrom('Search online')
    await fireEvent.input(screen.getByLabelText('Keywords or GitHub URL'), {
      target: { value: 'https://github.com/team/skills' },
    })
    await fireEvent.click(screen.getByRole('button', { name: /^Search$/ }))
    await fireEvent.click(await screen.findByRole('button', { name: 'Preview skill' }))
    await screen.findByText('online skill body')
    expect(search).toHaveBeenCalledWith('https://github.com/team/skills', { signal: expect.any(AbortSignal) })
    expect(prepare).toHaveBeenCalledWith('https://github.com/team/skills/tree/main/writer', {
      signal: expect.any(AbortSignal),
    })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    await screen.findByLabelText('Keywords or GitHub URL')
    expect(install).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Preview skill' }))
    await screen.findByText('online skill body')
    await fireEvent.click(screen.getByRole('button', { name: /^Install$/ }))
    await waitFor(() => expect(install).toHaveBeenCalledWith(source, 'd-online skill body', false))
  })

  it('searches a user-selected system folder and previews only the selected authorized candidate', async () => {
    const candidate = {
      name: 'project-writer',
      path: 'E:/workspace/project/.claude/skills/writer',
      origin: 'Selected folder',
    }
    const source: Source = { bytes: { fileName: 'writer.zip', bytes: [7] } }
    const discover = vi.fn(async () => [])
    const pickSearchDirectory = vi.fn(async () => [candidate])
    const prepareSystem = vi.fn(async () => ({ source, origin: candidate.path }))
    const previewFn = vi.fn(async () => preview('system skill body'))
    const install = vi.fn()
    mount({ skillSources: { discover, pickSearchDirectory, prepareSystem }, skills: { preview: previewFn, install } })
    await addFrom('Search this computer')
    await waitFor(() => expect(discover).toHaveBeenCalledTimes(1))
    await fireEvent.click(await screen.findByRole('button', { name: 'Choose skill folder' }))
    await screen.findByText(/E:\/workspace\/project/)
    expect(prepareSystem).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Preview skill' }))
    await screen.findByText('system skill body')
    expect(prepareSystem).toHaveBeenCalledWith(candidate.path, { signal: expect.any(AbortSignal) })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
  })

  it('reviews the complete built-in creator before creating an authoring session', async () => {
    const api = authoring()
    mountAuthoring(api)
    await addFrom('Create with Agent')
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
      emit({ kind: 'end', status: 'failed', error: { code: 'needs_config', message: 'not configured' } })
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
    expect(screen.getByLabelText('Message to the skill creation assistant').hasAttribute('disabled')).toBe(false)
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
    const previewFn = vi.fn<(source: Source) => Promise<Preview>>(async () => preview('packaged skill body'))
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
    await waitFor(() => expect(api.approve).toHaveBeenCalledWith('authoring-1', 'approval-run', 'call-1', true))
    emit?.({ kind: 'tool_result', callId: 'call-1', outcome: 'ok', summary: 'validation passed' })
    await screen.findByText('shell: validation passed')
    emit?.({ kind: 'tool_call', callId: 'call-2', name: 'shell', source: 'host' })
    emit?.({ kind: 'approval_required', callId: 'call-2', name: 'shell', argsPreview: '{"command":"unsafe"}' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Deny' }))
    await waitFor(() => expect(api.approve).toHaveBeenCalledWith('authoring-1', 'approval-run', 'call-2', false))
  })

  it('cancels and closes authoring workspaces and ignores late run acceptance/output', async () => {
    const acceptance = Promise.withResolvers<{ runId: string; conversationId: null; turnId: null }>()
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
    await addFrom('Create with Agent')
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
    const api = authoring({ send: vi.fn(async () => ({ runId: 'active-run', conversationId: null, turnId: null })) })
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
    expect(screen.getByLabelText('Message to the skill creation assistant').hasAttribute('disabled')).toBe(false)
    expect(install).not.toHaveBeenCalled()
  })

  it('imports a selected local folder through the same preview boundary without installing', async () => {
    const source: Source = { bytes: { fileName: 'folder.zip', bytes: [4, 5] } }
    const pickDirectory = vi.fn(async () => ({ source, origin: 'E:/skills/writer' }))
    const previewFn = vi.fn(async () => preview('local folder body'))
    const install = vi.fn()
    mount({ skillSources: { pickDirectory }, skills: { preview: previewFn, install } })
    await addFrom('Import local file or folder')
    await fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }))
    await screen.findByText('local folder body')
    expect(pickDirectory).toHaveBeenCalledWith({ signal: expect.any(AbortSignal) })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
  })

  it('ignores preparation completed after its source dialog was closed', async () => {
    const pending = Promise.withResolvers<{ source: Source; origin: string }>()
    const pickDirectory = vi.fn(() => pending.promise)
    const previewFn = vi.fn()
    const install = vi.fn()
    mount({ skillSources: { pickDirectory }, skills: { preview: previewFn, install } })
    await addFrom('Import local file or folder')
    await fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }))
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    pending.resolve({ source: { bytes: { fileName: 'late.zip', bytes: [9] } }, origin: 'late folder' })
    await pending.promise
    await addFrom('Import local file or folder')
    expect(previewFn).not.toHaveBeenCalled()
    expect(install).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: /^Install$/ })).toBeNull()
  })

  it('requires an explicit authoring model without sending or saving', async () => {
    const api = authoring()
    const save = vi.fn()
    mountAuthoring(api, { skills: { save } })
    await startAuthoring()
    await authorMessage('Create a writer')
    expect((await screen.findByRole('alert')).textContent).toBe('Choose a configured chat model before sending')
    expect(api.send).not.toHaveBeenCalled()
    expect(save).not.toHaveBeenCalled()
  })

  it('hides Agent when the host has no model capability, retaining online, system and local sources', async () => {
    mount({ omit: ['providers'], skillSources: { authoring: authoring() } })
    await fireEvent.click(screen.getByRole('button', { name: 'Add skill' }))
    expect(screen.queryByRole('menuitem', { name: 'Create with Agent' })).toBeNull()
    expect(await screen.findByRole('menuitem', { name: 'Search online' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Search this computer' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Import local file or folder' })).toBeTruthy()
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
      return Promise.resolve({ runId: 'r2', conversationId: null, turnId: null })
    })
    const get = vi.fn(() => Promise.resolve({ ...meta(2), body: 'edited body', license: null, extra: {}, files: [] }))
    mount({ chat: { start: run }, skills: { list: () => Promise.resolve([meta(2)]), get } }, [
      { id: 's1', revision: 1 },
    ])

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
