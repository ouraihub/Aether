import { render, screen, fireEvent } from '@testing-library/svelte'
import { createI18n } from '../../i18n'
import {
  createFakePorts,
  type FakePortsOptions,
  type Preview,
  type RunEvent,
  type SkillAuthoringPort,
  type SkillDto,
  type SkillMeta,
  type Source,
} from '../../ports'
import { vi } from 'vitest'
import Harness from '../Harness.test.svelte'

export const meta = (
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

export const preview = (body: string): Preview => ({
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

export function mount(options: FakePortsOptions, skills?: { id: string; revision: number }[]) {
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

export async function addFrom(name: string) {
  await fireEvent.click(screen.getByRole('button', { name: 'Add skill' }))
  await fireEvent.click(await screen.findByRole('menuitem', { name }))
}

export async function selectAuthoringModel() {
  await fireEvent.click(await screen.findByLabelText('Select a model'))
  await fireEvent.click(await screen.findByRole('option', { name: /m1/ }))
}

export const creator: SkillDto = {
  ...meta(1),
  id: 'builtin-creator',
  name: 'skill-creator',
  origin: 'builtin',
  body: 'Complete creator instructions: capture intent, draft, evaluate, improve, package.',
  license: null,
  extra: {},
  files: [{ path: 'scripts/quick_validate.py', kind: 'script', size: 10 }],
}

export function authoring(overrides: Partial<SkillAuthoringPort> = {}): SkillAuthoringPort {
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

export function mountAuthoring(api: SkillAuthoringPort, options: FakePortsOptions = {}) {
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

export async function startAuthoring() {
  await addFrom('Create with Agent')
  await fireEvent.click(await screen.findByRole('button', { name: 'Review skill-creator' }))
  await screen.findByText(creator.body)
  await fireEvent.click(screen.getByRole('button', { name: 'Confirm skill-creator and start' }))
  await screen.findByLabelText('Message to the skill creation assistant')
}

export async function authorMessage(text: string) {
  await fireEvent.input(screen.getByLabelText('Message to the skill creation assistant'), {
    target: { value: text },
  })
  await fireEvent.click(screen.getByRole('button', { name: /^Send$/ }))
}

export type { Preview, RunEvent, SkillAuthoringPort, Source }
