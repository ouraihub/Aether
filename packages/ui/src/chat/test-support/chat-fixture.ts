import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import type { ComponentProps } from 'svelte'
import { expect, vi } from 'vitest'
import { createI18n } from '../../i18n'
import {
  createFakePorts,
  KitClientError,
  type ConversationClient,
  type Conversation,
  type RunClient,
  type RunEvent,
  type RunRequest,
  type ProvidersPort,
  type SearchPort,
  type SkillsPort,
} from '../../ports'
import type ChatPanel from '../ChatPanel.svelte'
import Harness from '../Harness.test.svelte'

export const model = { endpointId: 'e1', modelId: 'm1' }

export function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected chat fixture or control is missing')
  return value
}

export interface Run {
  request: RunRequest
  turnId: string
  emit: (event: RunEvent) => void
  signal: AbortSignal | undefined
}

export interface Extra {
  props?: Partial<ComponentProps<typeof ChatPanel>>
  history?: Partial<ConversationClient>
  providers?: Partial<ProvidersPort>
  search?: Partial<SearchPort>
  skills?: Partial<SkillsPort>
  chat?: Partial<RunClient>
}

export function setup(
  withHistory: boolean,
  context: { kind: string; id: string; label: string } | null = null,
  extra: Extra = {},
) {
  const runs: Run[] = []
  const stored: Conversation = {
    summary: {
      id: 'c1',
      title: 'Test',
      scope: null,
      selectedTurnId: null,
      createdAt: 0,
      updatedAt: 0,
    },
    turns: [],
  }
  const approve = vi.fn<(runId: string, callId: string, allow: boolean) => Promise<void>>(() =>
    Promise.resolve(),
  )
  const ports = createFakePorts({
    providers: {
      modelOptions: () =>
        Promise.resolve([
          {
            ...model,
            label: 'Local · main · m1',
            modelType: 'chat',
            name: null,
            supportsTools: null,
            reasoning: null,
            nativeWebSearch: false,
            canVision: false,
          },
        ]),
      ...extra.providers,
    },
    ...(extra.search ? { search: extra.search } : {}),
    ...(extra.skills ? { skills: extra.skills } : {}),
    chat: {
      start: (request, emit, options) => {
        const id = crypto.randomUUID()
        const source = stored.turns.find(
          (turn) => turn.id === (request.retryTurnId ?? request.translateTurnId),
        )
        const turn: Conversation['turns'][number] = {
          id,
          parentId: request.retryTurnId
            ? (source?.parentId ?? null)
            : (request.translateTurnId ?? request.replyToId ?? stored.summary.selectedTurnId),
          questionId: request.retryTurnId ? (source?.questionId ?? id) : id,
          supersedesId: request.retryTurnId ?? null,
          kind: request.translateTurnId ? 'translation' : 'reply',
          input: source?.input ?? request.input,
          images: source
            ? source.images
            : (request.images ?? []).map((image) => ({
                mime: image.mime,
                url: `data:${image.mime};base64,${image.data}`,
              })),
          generated: [],
          text: '',
          status: 'running',
          errorCode: null,
          endpointId: request.model?.endpointId ?? null,
          modelId: request.model?.modelId ?? null,
          reasoningEffort: request.reasoningEffort ?? 'default',
          reasoningText: null,
        }
        stored.turns.push(turn)
        const wrapped = (event: RunEvent) => {
          if (event.kind === 'text_delta') turn.text += event.text
          if (event.kind === 'reasoning_delta')
            turn.reasoningText = (turn.reasoningText ?? '') + event.text
          if (event.kind === 'usage') {
            turn.usage = {
              inputTokens: event.inputTokens,
              outputTokens: event.outputTokens,
              cacheReadTokens: event.cacheReadTokens,
              cacheWriteTokens: event.cacheWriteTokens,
              reasoningTokens: event.reasoningTokens,
            }
            turn.cost = event.cost
          }
          if (event.kind === 'context') turn.contextUsage = event.usage
          if (event.kind === 'end') {
            turn.status = event.status
            turn.errorCode = event.error?.code ?? null
            if (event.status === 'completed' && !request.retryTurnId)
              stored.summary.selectedTurnId = id
          }
          emit(event)
        }
        runs.push({ request, turnId: id, emit: wrapped, signal: options?.signal })
        options?.signal?.addEventListener('abort', () =>
          wrapped({ kind: 'end', status: 'cancelled' }),
        )
        return Promise.resolve({
          runId: `r${runs.length}`,
          conversationId: withHistory ? (request.conversationId ?? 'c1') : null,
          turnId: withHistory ? id : null,
        })
      },
      approve,
      ...extra.chat,
    },
  })
  if (ports.conversations)
    Object.assign(
      ports.conversations,
      {
        get: (id: string) =>
          id === stored.summary.id
            ? Promise.resolve(structuredClone(stored))
            : Promise.reject(
                new KitClientError({ code: 'not_found', message: 'conversation not found' }),
              ),
      },
      extra.history,
    )
  if (!withHistory) delete ports.conversations
  const notify = { toast: vi.fn(), confirm: vi.fn(() => Promise.resolve(true)) }
  const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const kit = { ports, i18n: createI18n('en'), notify, logger }
  const view = render(Harness, { props: { kit, model, context, ...extra.props } })
  return { runs, view, approve, notify }
}

export async function send(text: string) {
  await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: text },
  })
  const button = screen.getByRole('button', { name: 'Send' })
  await waitFor(() => expect(button.hasAttribute('disabled')).toBe(false))
  await fireEvent.click(button)
}

const SEARCH = 'Search models, optionally with a level'

/** The composer model/effort pill; its name carries the current model and effort. */
export const pickerTrigger = () => screen.findByRole('button', { name: /^Model and reasoning:/ })

/** Opens the picker, searches so every group unfolds, picks a row, then closes it like a user. */
export async function pickModel(search: string, name: RegExp) {
  const trigger = await pickerTrigger()
  if (trigger.getAttribute('aria-expanded') !== 'true') await fireEvent.click(trigger)
  const input = await screen.findByLabelText(SEARCH)
  await fireEvent.input(input, { target: { value: search } })
  await fireEvent.click(await screen.findByRole('option', { name }))
  await fireEvent.keyDown(input, { key: 'Escape' })
  await waitFor(() => expect(screen.queryByLabelText(SEARCH)).toBeNull())
}
