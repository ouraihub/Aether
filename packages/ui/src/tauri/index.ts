// The only directory allowed to import @tauri-apps/* (ESLint enforces): createTauriPorts() (docs/16 §2).
// Argument names are the camelCase of the Rust command parameters (docs/15 §5).
import { Channel, invoke } from '@tauri-apps/api/core'
import { KitClientError } from '../ports'
import type { DesktopActions, KitError, KitPorts, LogsPort, RunAccepted, RunEvent, SearchPort } from '../ports'

/** Which feature ports the host enabled; the rest stay absent so their UI areas are hidden. */
export interface TauriPortsOptions {
  providers?: boolean
  search?: boolean
  chat?: boolean
  /** Independent conversation storage capability (`aether:chat-history`). */
  conversations?: boolean
  skills?: boolean
  mcpClient?: boolean
  mcpServer?: boolean
}

const CANCELLED: KitError = { code: 'cancelled', message: 'cancelled' }

/** Invokes `plugin:aether|<command>` and turns any rejection into a KitClientError at the boundary. */
async function call<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  try {
    return await invoke<T>(`plugin:aether|${command}`, args)
  } catch (error) {
    const isKitError =
      typeof error === 'object' &&
      error !== null &&
      typeof (error as Partial<KitError>).code === 'string' &&
      typeof (error as Partial<KitError>).message === 'string'
    throw new KitClientError(isKitError ? (error as KitError) : { code: 'internal', message: String(error) })
  }
}

/** The plugin always registers the log commands, so desktop hosts always get `logs`. */
export function createTauriPorts(
  options: TauriPortsOptions = {},
): KitPorts & { logs: LogsPort; desktop: DesktopActions } {
  const ports: KitPorts & { logs: LogsPort; desktop: DesktopActions } = {
    logs: {
      tail: (limit) => call('logs_tail', { limit }),
      ingest: (batch) => call('logs_ingest', { batch }),
      getLevel: () => call('logs_get_level'),
      setLevel: (level) => call('logs_set_level', { level }),
    },
    desktop: { openLogsDirectory: () => call('logs_open_dir') },
  }
  if (options.providers) {
    ports.providers = {
      list: () => call('providers_list'),
      setEnabled: (id, enabled) => call('providers_set_enabled', { id, enabled }),
      save: (input) => call('providers_save', { input }),
      remove: (id) => call('providers_delete', { id }),
      modelOptions: (filter = {}) => call('providers_model_options', { filter }),
      // Aborting calls providers_cancel; the pending probe then rejects with `cancelled`.
      probe: async (target, callOptions) => {
        const signal = callOptions?.signal
        if (signal?.aborted) throw new KitClientError(CANCELLED)
        const jobId = crypto.randomUUID()
        const cancel = () => void call('providers_cancel', { jobId }).catch(() => undefined)
        signal?.addEventListener('abort', cancel, { once: true })
        try {
          return await call('providers_probe', { jobId, target })
        } finally {
          signal?.removeEventListener('abort', cancel)
        }
      },
      revealNeedsPassword: false,
      keyStrategies: false,
      saveKey: (input) => call('providers_key_save', { input }),
      removeKey: (endpointId, keyId) => call('providers_key_delete', { endpointId, keyId }),
      revealKey: (keyId) => call('providers_key_reveal', { keyId }),
    }
  }
  if (options.search) {
    const searchCall = async <T>(
      command: string,
      args: Record<string, unknown> = {},
      callOptions?: { signal?: AbortSignal },
    ): Promise<T> => {
      const signal = callOptions?.signal
      if (signal?.aborted) throw new KitClientError(CANCELLED)
      const jobId = crypto.randomUUID()
      const cancel = () => void call('search_cancel', { jobId }).catch(() => undefined)
      signal?.addEventListener('abort', cancel, { once: true })
      try {
        return await call<T>(command, { ...args, jobId })
      } finally {
        signal?.removeEventListener('abort', cancel)
      }
    }
    const search: SearchPort = {
      settings: (capability, o) => searchCall('search_settings', { capability }, o),
      save: (input, o) => searchCall('search_save', { input }, o),
      setDefault: (capability, provider, o) => searchCall('search_set_default', { capability, provider }, o),
      savePreferences: (input, o) => searchCall('search_save_preferences', { input }, o),
      saveKey: (capability, provider, value, keyId, o) =>
        searchCall('search_save_key', { capability, provider, value, keyId: keyId ?? null }, o),
      removeKey: (capability, provider, keyId, o) =>
        searchCall('search_remove_key', { capability, provider, keyId }, o),
      reveal: (capability, provider, keyId, currentPassword, o) =>
        searchCall('search_reveal', { capability, provider, keyId, currentPassword: currentPassword ?? null }, o),
      check: (capability, provider, o) => searchCall('search_check', { capability, provider }, o),
      search: (query, o) => searchCall('search_query', { query }, o),
      fetch: (url, o) => searchCall('search_fetch', { url }, o),
    }
    ports.search = search
  }
  if (options.chat) {
    ports.chat = {
      start: async (request, onEvent, callOptions) => {
        const signal = callOptions?.signal
        if (signal?.aborted) throw new KitClientError(CANCELLED)
        let accepted: RunAccepted | null = null
        let ended = false
        let requested = false
        let dispatched = false
        const cleanup = () => signal?.removeEventListener('abort', cancel)
        const dispatchCancel = () => {
          if (!accepted || ended || dispatched) return
          dispatched = true
          void call('chat_cancel', { runId: accepted.runId }).catch(() => undefined)
        }
        const cancel = () => {
          requested = true
          dispatchCancel()
        }
        signal?.addEventListener('abort', cancel, { once: true })
        const channel = new Channel<RunEvent>((event) => {
          if (ended) return
          if (event.kind === 'end') {
            ended = true
            cleanup()
          }
          onEvent(event)
        })
        try {
          accepted = await call<RunAccepted>('chat_run', { request, onEvent: channel })
          if (requested || signal?.aborted) dispatchCancel()
          return accepted
        } catch (error) {
          ended = true
          cleanup()
          throw error
        }
      },
      cancel: (runId) => call('chat_cancel', { runId }),
      getSnapshot: (runId) => call('chat_snapshot', { runId }),
      approve: async (runId, callId, allow) => {
        await call('chat_approve', { runId, callId, allow })
      },
    }
  }
  if (options.conversations) {
    ports.conversations = {
      list: (scope) => call('chat_conversations', { scope: scope ?? null }),
      get: (id) => call('chat_conversation', { id }),
      remove: async (id) => {
        await call('chat_delete', { id })
      },
      rename: (id, title) => call('chat_rename', { id, title }),
      editTurn: (conversationId, turnId, text, expectedText) =>
        call('chat_edit_turn', { conversationId, turnId, text, expectedText }),
      removeTurn: (conversationId, turnId) => call('chat_remove_turn', { conversationId, turnId }),
      fork: (conversationId, turnId) => call('chat_fork', { conversationId, turnId }),
      selectTurn: (conversationId, turnId) => call('chat_select_turn', { conversationId, turnId }),
    }
  }
  if (options.skills) {
    ports.skills = {
      list: () => call('skills_list'),
      get: (id) => call('skills_get', { id }),
      save: (input) => call('skills_save', { input }),
      preview: (source) => call('skills_preview', { source }),
      install: (source, confirmedDigest, replace) => call('skills_install', { source, confirmedDigest, replace }),
      duplicate: (id, newName) => call('skills_duplicate', { id, newName }),
      reorder: (ids) => call('skills_reorder', { ids }),
      remove: (id) => call('skills_delete', { id }),
    }
  }
  if (options.mcpClient) {
    ports.mcpClient = {
      list: () => call('mcp_client_list'),
      save: (input) => call('mcp_client_save', { input }),
      remove: (id) => call('mcp_client_delete', { id }),
      refresh: (serverId) => call('mcp_client_refresh', { serverId }),
      setToolApproval: async (serverId, tool, approval) => {
        await call('mcp_client_set_tool_approval', { serverId, tool, approval })
      },
    }
  }
  if (options.mcpServer) {
    ports.mcpServer = {
      status: () => call('mcp_server_status'),
      tokens: () => call('mcp_server_tokens'),
      start: () => call('mcp_server_start'),
      stop: () => call('mcp_server_stop'),
      setPort: (port) => call('mcp_server_set_port', { port }),
      issueToken: (name, scopes) => call('mcp_server_issue_token', { name, scopes }),
      revokeToken: async (id) => {
        await call('mcp_server_revoke_token', { id })
      },
      scopes: () => call('mcp_server_scopes'),
      setScopes: (ids) => call('mcp_server_set_scopes', { ids }),
    }
  }
  return ports
}
