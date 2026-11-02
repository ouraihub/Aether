import { vi } from 'vitest'
import { createFakePorts } from '../../ports'
import type {
  ConversationClient,
  Conversation,
  RunAccepted,
  RunEvent,
  RunRequest,
  Turn,
} from '../../ports'
import { RunSession, type RunBase } from '../run-session.svelte'

export const sourceModel = { endpointId: 'original-endpoint', modelId: 'original-model' }
export const pickedModel = { endpointId: 'picked-endpoint', modelId: 'picked-model' }
export const base: RunBase = {
  purpose: 'chat',
  scope: null,
  language: 'en',
  model: pickedModel,
  skills: [],
  context: null,
}

export function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error('Expected run-session fixture is missing')
  return value
}

export function stored(overrides: Partial<Turn> = {}): Turn {
  return {
    id: 'source',
    parentId: null,
    questionId: 'question',
    supersedesId: null,
    kind: 'reply',
    endpointId: sourceModel.endpointId,
    modelId: sourceModel.modelId,
    input: 'Original question',
    text: 'Original **answer**',
    status: 'completed',
    errorCode: null,
    reasoningText: 'Original reasoning',
    reasoningEffort: 'default',
    images: [],
    generated: [],
    ...overrides,
  }
}

export function conversation(
  turns: Turn[],
  selectedTurnId: string | null = 'source',
): Conversation {
  return {
    summary: {
      id: 'conversation',
      title: 'Conversation',
      scope: null,
      createdAt: 0,
      updatedAt: 0,
      selectedTurnId,
    },
    turns,
  }
}

export interface Deferred<T> {
  promise: Promise<T>
  resolve: (value: T) => void
  reject: (error: unknown) => void
}
export function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

export function setup(withHistory = true) {
  const runs: {
    request: RunRequest
    emit: (event: RunEvent) => void
    accepted: Deferred<RunAccepted>
  }[] = []
  const get = vi.fn<ConversationClient['get']>(() => Promise.resolve(conversation([stored()])))
  const cancel = vi.fn(() => Promise.resolve())
  const approve = vi.fn(() => Promise.resolve())
  const ports = createFakePorts({
    chat: {
      cancel,
      approve,
      start: (request, emit) => {
        const accepted = deferred<RunAccepted>()
        runs.push({ request, emit, accepted })
        return accepted.promise
      },
    },
  })
  const chat = required(ports.chat)
  if (withHistory) Object.assign(required(ports.conversations), { get })
  else delete ports.conversations
  return { session: new RunSession(chat, ports.conversations), runs, get, cancel, approve }
}

export const accepted: RunAccepted = {
  runId: 'run',
  conversationId: 'conversation',
  turnId: 'candidate',
}
