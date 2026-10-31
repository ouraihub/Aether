import type { CallOptions } from './contract'

import type {
  SearchCap,
  SearchPreferences,
  SearchSettingsDto,
  SearchSave,
  SearchResult,
  SearchPage,
} from '@ouraihub/aether-contracts'
export type {
  SearchCap,
  SearchKey,
  SearchPreferences,
  SearchProvider,
  SearchSettingsDto,
  SearchSave,
  SearchResult,
  SearchHit,
  SearchPage,
} from '@ouraihub/aether-contracts'

export interface SearchPort {
  revealNeedsPassword?: boolean
  settings(
    capability: SearchCap,
    options?: CallOptions,
  ): Promise<SearchSettingsDto>
  save(input: SearchSave, options?: CallOptions): Promise<SearchSettingsDto>
  setDefault(
    capability: SearchCap,
    provider: string,
    options?: CallOptions,
  ): Promise<SearchSettingsDto>
  savePreferences(
    input: SearchPreferences,
    options?: CallOptions,
  ): Promise<SearchPreferences>
  saveKey(
    capability: SearchCap,
    provider: string,
    value: string,
    keyId?: string,
    options?: CallOptions,
  ): Promise<SearchSettingsDto>
  removeKey(
    capability: SearchCap,
    provider: string,
    keyId: string,
    options?: CallOptions,
  ): Promise<SearchSettingsDto>
  reveal(
    capability: SearchCap,
    provider: string,
    keyId: string,
    currentPassword?: string,
    options?: CallOptions,
  ): Promise<string>
  check(
    capability: SearchCap,
    provider: string,
    options?: CallOptions,
  ): Promise<{ count: number }>
  search(query: string, options?: CallOptions): Promise<SearchResult>
  fetch(url: string, options?: CallOptions): Promise<SearchPage>
}
