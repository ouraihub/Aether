import type { CallOptions } from './contract'

export type SearchCap = 'search' | 'fetch'

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
  apiKeyMasked?: string
  basicAuthUsername?: string
  ready: boolean
}

export interface SearchSettingsDto {
  capability: SearchCap
  defaultProvider: string
  maxResults?: number
  providers: SearchProvider[]
}

export interface SearchSave {
  capability: SearchCap
  provider: string
  baseUrl?: string
  /** Omitted = keep; empty = delete; nonempty = replace. */
  apiKey?: string
  basicAuthUsername?: string
}

export interface SearchHit {
  title: string
  url: string
  snippet: string
}

export interface SearchResult {
  query: string
  provider: string
  hits: SearchHit[]
}

export interface SearchPage {
  title: string
  url: string
  content: string
}

export interface SearchPort {
  /** Hosts requiring password verification before revealing a stored key set this flag. */
  revealNeedsPassword?: boolean
  settings(capability: SearchCap, options?: CallOptions): Promise<SearchSettingsDto>
  save(input: SearchSave, options?: CallOptions): Promise<SearchSettingsDto>
  setDefault(
    capability: SearchCap,
    provider: string,
    maxResults?: number,
    options?: CallOptions,
  ): Promise<SearchSettingsDto>
  reveal(capability: SearchCap, provider: string, currentPassword?: string, options?: CallOptions): Promise<string>
  check(capability: SearchCap, provider: string, options?: CallOptions): Promise<{ count: number }>
  search(query: string, options?: CallOptions): Promise<SearchResult>
  fetch(url: string, options?: CallOptions): Promise<SearchPage>
}
