// Complete in-memory ports for tests and demos (docs/16 §6). Override any method; omit a port with `omit`.
import { KitClientError } from './contract'
import type { ChatPort, KitPorts, LogsPort, McpClientPort, McpServerPort, ProvidersPort, SkillsPort } from './contract'
import type { LogLevel } from './generated/LogLevel'

type PortName = Exclude<keyof KitPorts, 'logs'>

export interface FakePortsOptions {
  logs?: Partial<LogsPort>
  providers?: Partial<ProvidersPort>
  chat?: Partial<ChatPort>
  skills?: Partial<SkillsPort>
  mcpClient?: Partial<McpClientPort>
  mcpServer?: Partial<McpServerPort>
  /** Ports to leave out, as a host without that feature would. */
  omit?: readonly PortName[]
}

const STOPPED = { running: false, port: 0, url: '', toolCount: 0 }

export function createFakePorts(options: FakePortsOptions = {}): KitPorts {
  let level: LogLevel = 'INFO'
  const ports: Required<KitPorts> = {
    logs: {
      tail: () => Promise.resolve([]),
      ingest: (batch) => Promise.resolve({ accepted: batch.entries.length, rejected: 0 }),
      openDir: () => Promise.resolve(),
      getLevel: () => Promise.resolve({ level, available: true }),
      setLevel: (next) => {
        level = next
        return Promise.resolve({ level, available: true })
      },
      ...options.logs,
    },
    providers: {
      list: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      modelOptions: () => Promise.resolve([]),
      probe: () => Promise.resolve({ probeUrl: '', models: [], error: null }),
      ...options.providers,
    },
    chat: {
      // Accept, then end the run once, like the real backend.
      run: (_request, onEvent, callOptions) => {
        if (callOptions?.signal?.aborted) {
          return Promise.reject(new KitClientError({ code: 'cancelled', message: 'cancelled' }))
        }
        queueMicrotask(() => onEvent({ kind: 'end', status: 'completed' }))
        return Promise.resolve({ runId: 'fake-run', conversationId: null })
      },
      approve: () => Promise.resolve(),
      // `delete ports.chat.history` for a host without a conversation store.
      history: {
        list: () => Promise.resolve([]),
        get: (id) =>
          Promise.reject(
            new KitClientError({
              code: 'not_found',
              message: `conversation ${id}`,
              details: { entity: 'conversation' },
            }),
          ),
        remove: () => Promise.resolve(),
        rename: (id, title) => Promise.resolve({ id, title, scope: null, createdAt: 0, updatedAt: 0 }),
      },
      ...options.chat,
    },
    skills: {
      list: () => Promise.resolve([]),
      get: (id) =>
        Promise.reject(new KitClientError({ code: 'not_found', message: `skill ${id}`, details: { entity: 'skill' } })),
      preview: () =>
        Promise.reject(
          new KitClientError({ code: 'bad_request', message: 'no package', details: { field: 'SKILL.md' } }),
        ),
      install: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      duplicate: () => Promise.resolve([]),
      reorder: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      ...options.skills,
    },
    mcpClient: {
      list: () => Promise.resolve([]),
      save: () => Promise.resolve([]),
      remove: () => Promise.resolve([]),
      refresh: () => Promise.resolve([]),
      setToolApproval: () => Promise.resolve(),
      ...options.mcpClient,
    },
    mcpServer: {
      status: () => Promise.resolve(STOPPED),
      tokens: () => Promise.resolve([]),
      start: () => Promise.resolve(STOPPED),
      stop: () => Promise.resolve(STOPPED),
      setPort: (port) => Promise.resolve({ ...STOPPED, port }),
      issueToken: (name, scopes) =>
        Promise.resolve({
          token: 'kit_fake',
          id: 'fake-token',
          name,
          scopes: [...scopes],
          createdAt: 0,
          lastUsedAt: null,
          revokedAt: null,
        }),
      revokeToken: () => Promise.resolve(),
      scopes: () => Promise.resolve([]),
      setScopes: () => Promise.resolve([]),
      ...options.mcpServer,
    },
  }
  const result: KitPorts = { ...ports }
  for (const name of options.omit ?? []) delete result[name]
  return result
}
