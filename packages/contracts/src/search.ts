import type { SearchHit } from './generated/SearchHit'

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
export interface SearchPage {
  title: string
  url: string
  content: string
}
