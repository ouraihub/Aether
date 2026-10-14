import type { CallOptions } from './contract'

export type SearchCap = 'search' | 'fetch'

export interface SearchKey {
  id: string
  masked: string
}

export interface SearchPreferences {
  maxResults: number
  compression: 'none' | 'cutoff'
  cutoffLimit: number
  blacklist: string[]
  modelToolsPreferred: boolean
}

export interface SearchProvider {
  id: string
  name: string
  description: string
  officialWebsite?: string
  apiKeyWebsite?: string
  keyMode: 'required' | 'optional' | 'none'
  basicAuth: boolean
  defaultBaseUrl?: string
  baseUrl?: string
  basicAuthUsername?: string
  keys: SearchKey[]
  ready: boolean
}

export interface SearchSettingsDto {
  capability: SearchCap
  defaultProvider: string
  providers: SearchProvider[]
  preferences: SearchPreferences
}

export interface SearchSave {
  capability: SearchCap
  provider: string
  baseUrl?: string
  basicAuthUsername?: string
}

export interface SearchResult {
  query: string
  provider: string
  hits: SearchHit[]
}

export interface SearchHit {
  title: string
  url: string
  snippet: string
}

export interface SearchPage {
  title: string
  url: string
  content: string
}

export interface SearchPort {
  revealNeedsPassword?: boolean
  settings(capability: SearchCap, options?: CallOptions): Promise<SearchSettingsDto>
  save(input: SearchSave, options?: CallOptions): Promise<SearchSettingsDto>
  setDefault(capability: SearchCap, provider: string, options?: CallOptions): Promise<SearchSettingsDto>
  savePreferences(input: SearchPreferences, options?: CallOptions): Promise<SearchPreferences>
  saveKey(
    capability: SearchCap,
    provider: string,
    value: string,
    keyId?: string,
    options?: CallOptions,
  ): Promise<SearchSettingsDto>
  removeKey(capability: SearchCap, provider: string, keyId: string, options?: CallOptions): Promise<SearchSettingsDto>
  reveal(
    capability: SearchCap,
    provider: string,
    keyId: string,
    currentPassword?: string,
    options?: CallOptions,
  ): Promise<string>
  check(capability: SearchCap, provider: string, options?: CallOptions): Promise<{ count: number }>
  search(query: string, options?: CallOptions): Promise<SearchResult>
  fetch(url: string, options?: CallOptions): Promise<SearchPage>
}
