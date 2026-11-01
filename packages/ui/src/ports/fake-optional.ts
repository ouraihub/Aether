import type { KitClientError } from './contract'
import type { McpClientPort, McpServerPort } from './contract'
import type { SearchPort } from './search'
import type { SkillSourcesPort } from './skill-sources'

export type FakeError = (
  code: ConstructorParameters<typeof KitClientError>[0]['code'],
  message: string,
  details?: Record<string, string>,
) => KitClientError

const STOPPED = { running: false, port: 0, url: '', toolCount: 0 }

export function mcpClientDefaults(options: Partial<McpClientPort>): McpClientPort {
  return {
    list: () => Promise.resolve([]),
    save: () => Promise.resolve([]),
    remove: () => Promise.resolve([]),
    refresh: () => Promise.resolve([]),
    setToolApproval: () => Promise.resolve(),
    ...options,
  }
}

export function mcpServerDefaults(options: Partial<McpServerPort>): McpServerPort {
  return {
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
    ...options,
  }
}

export function searchDefaults(options: Partial<SearchPort>, error: FakeError): SearchPort {
  const unavailable = () => Promise.reject(error('internal', 'Configure the fake search method'))
  return {
    settings: unavailable,
    save: unavailable,
    setDefault: unavailable,
    savePreferences: unavailable,
    saveKey: unavailable,
    removeKey: unavailable,
    reveal: unavailable,
    check: unavailable,
    search: unavailable,
    fetch: unavailable,
    ...options,
  }
}

export function skillSourceDefaults(
  options: Partial<SkillSourcesPort>,
  error: FakeError,
): SkillSourcesPort {
  const unavailable = () =>
    Promise.reject(error('internal', 'Configure the fake skill source method'))
  return {
    search: unavailable,
    prepare: unavailable,
    discover: unavailable,
    prepareSystem: unavailable,
    pickDirectory: unavailable,
    pickSearchDirectory: unavailable,
    ...options,
  }
}
