import type { SearchCap, SearchHit, SearchPreferences } from '@ouraihub/aether-contracts'
import { badRequest } from '../error.js'

export interface ProviderSpec {
  id: string
  name: string
  description: string
  basicAuth: boolean
  search?: { defaultBaseUrl: string; keyMode: 'required' | 'optional' | 'none' }
  fetch?: { defaultBaseUrl: string; keyMode: 'required' | 'optional' | 'none' }
}
export const PROVIDERS: readonly ProviderSpec[] = [
  {
    id: 'zhipu',
    name: '智谱 Zhipu',
    description: 'Web search',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://open.bigmodel.cn/api/paas/v4/web_search', keyMode: 'required' },
  },
  {
    id: 'tavily',
    name: 'Tavily',
    description: 'Web search',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.tavily.com', keyMode: 'required' },
  },
  {
    id: 'searxng',
    name: 'SearXNG',
    description: 'Self-hosted metasearch',
    basicAuth: true,
    search: { defaultBaseUrl: 'http://localhost:8080', keyMode: 'none' },
  },
  {
    id: 'exa',
    name: 'Exa',
    description: 'Semantic web search',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.exa.ai', keyMode: 'required' },
  },
  {
    id: 'exa-mcp',
    name: 'Exa MCP',
    description: 'Exa search over MCP; anonymous access is upstream-limited',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://mcp.exa.ai/mcp', keyMode: 'optional' },
  },
  {
    id: 'bocha',
    name: '博查 Bocha',
    description: 'Web search',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.bochaai.com', keyMode: 'required' },
  },
  {
    id: 'querit',
    name: 'Querit',
    description: 'Search and page content',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.querit.ai', keyMode: 'required' },
    fetch: { defaultBaseUrl: 'https://api.querit.ai', keyMode: 'required' },
  },
  {
    id: 'brave',
    name: 'Brave Search',
    description: 'Independent web search',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.search.brave.com', keyMode: 'required' },
  },
  {
    id: 'parallel',
    name: 'Parallel',
    description: 'Web search and excerpts',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.parallel.ai', keyMode: 'required' },
  },
  {
    id: 'jina',
    name: 'Jina',
    description: 'Search and page reader',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://s.jina.ai', keyMode: 'required' },
    fetch: { defaultBaseUrl: 'https://r.jina.ai', keyMode: 'optional' },
  },
  {
    id: 'firecrawl',
    name: 'Firecrawl',
    description: 'Search and Markdown scraping',
    basicAuth: false,
    search: { defaultBaseUrl: 'https://api.firecrawl.dev', keyMode: 'optional' },
    fetch: { defaultBaseUrl: 'https://api.firecrawl.dev', keyMode: 'optional' },
  },
  {
    id: 'fetch',
    name: 'Direct fetch',
    description: 'Static HTML and text; does not execute scripts',
    basicAuth: false,
    fetch: { defaultBaseUrl: '', keyMode: 'none' },
  },
]
export const DEFAULT_PROVIDERS: Readonly<Record<SearchCap, string>> = { search: 'exa-mcp', fetch: 'jina' }
export const defaultPreferences = (): SearchPreferences => ({
  maxResults: 5,
  compression: 'none',
  cutoffLimit: 2000,
  blacklist: [],
  modelToolsPreferred: false,
})
export function providerFor(cap: SearchCap, id: string) {
  const spec = PROVIDERS.find((item) => item.id === id)
  const config = spec?.[cap]
  if (!spec || !config) throw badRequest('provider', 'unsupported')
  return { spec, config }
}
export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
export function fields(value: unknown, allowed: readonly string[]): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some((key) => !allowed.includes(key)))
    throw badRequest('input', 'unknown_fields')
  return value
}
export function stringValue(value: unknown, field: string): string {
  if (typeof value !== 'string') throw badRequest(field, 'invalid')
  return value
}
export function capability(value: unknown): SearchCap {
  if (value !== 'search' && value !== 'fetch') throw badRequest('capability', 'invalid')
  return value
}
export function targetUrl(value: string): URL {
  let url: URL
  try {
    url = new URL(value)
  } catch {
    throw badRequest('url', 'invalid')
  }
  if (!['https:', 'http:'].includes(url.protocol) || !url.hostname || url.username || url.password)
    throw badRequest('url', 'invalid')
  url.hostname = url.hostname.toLowerCase().replace(/\.$/, '')
  return url
}
export function normalizeRule(value: unknown): string {
  if (typeof value !== 'string' || !value || [...value].length > 256 || /[\s[\]\\^{}|#]/u.test(value))
    throw badRequest('blacklist', 'invalid')
  for (const character of value) {
    const code = character.codePointAt(0)
    if (code !== undefined && (code <= 31 || code === 127)) throw badRequest('blacklist', 'invalid')
  }
  const parts = value.match(/^(https?):\/\/([^/]+)(\/.*)?$/i)
  const hostPort = parts ? (parts[2] ?? '') : value
  const match = hostPort.match(/^([^:]+)(?::([0-9]+))?$/)
  if (!match || (!parts && match[2]) || (match[2] && (Number(match[2]) < 1 || Number(match[2]) > 65535)))
    throw badRequest('blacklist', 'invalid')
  const host = (match[1] ?? '').toLowerCase().replace(/\.$/, '')
  if (
    host.length > 253 ||
    host
      .split('.')
      .some((label) => !label || label.length > 63 || !/^[a-z0-9*?](?:[a-z0-9*?-]*[a-z0-9*?])?$/u.test(label))
  )
    throw badRequest('blacklist', 'invalid')
  const scheme = parts?.[1]?.toLowerCase()
  const port =
    match[2] && !((scheme === 'https' && match[2] === '443') || (scheme === 'http' && match[2] === '80'))
      ? ':' + match[2]
      : ''
  return parts ? scheme + '://' + host + port + (parts[3] ?? '/') : host
}
export function validatePreferences(value: unknown): SearchPreferences {
  const input = fields(value, ['maxResults', 'compression', 'cutoffLimit', 'blacklist', 'modelToolsPreferred'])
  const { maxResults, compression, cutoffLimit, blacklist, modelToolsPreferred } = input
  if (
    typeof maxResults !== 'number' ||
    !Number.isInteger(maxResults) ||
    maxResults < 1 ||
    maxResults > 50 ||
    (compression !== 'none' && compression !== 'cutoff') ||
    typeof cutoffLimit !== 'number' ||
    !Number.isInteger(cutoffLimit) ||
    cutoffLimit < 100 ||
    cutoffLimit > 20000 ||
    !Array.isArray(blacklist) ||
    blacklist.length > 32 ||
    typeof modelToolsPreferred !== 'boolean'
  )
    throw badRequest('preferences', 'invalid')
  return {
    maxResults,
    compression,
    cutoffLimit,
    blacklist: blacklist.map((rule: unknown) => normalizeRule(rule)),
    modelToolsPreferred,
  }
}
function glob(pattern: string, value: string): boolean {
  let p = 0
  let v = 0
  let star = -1
  let retry = 0
  while (v < value.length) {
    if (pattern[p] === '?' || pattern[p] === value[v]) {
      p++
      v++
    } else if (pattern[p] === '*') {
      star = p++
      retry = v
    } else if (star >= 0) {
      p = star + 1
      v = ++retry
    } else return false
  }
  while (pattern[p] === '*') p++
  return p === pattern.length
}
export function checkBlacklist(raw: string, preferences: SearchPreferences): void {
  const url = targetUrl(raw)
  if (
    preferences.blacklist.some((rule) =>
      rule.includes('://')
        ? glob(rule, url.href)
        : rule.includes('*') || rule.includes('?')
          ? glob(rule, url.hostname)
          : url.hostname === rule || url.hostname.endsWith('.' + rule),
    )
  )
    throw badRequest('url', 'blacklisted')
}
export function truncate(text: string, limit: number): string {
  let index = 0
  let count = 0
  for (const char of text) {
    if (count++ >= limit) break
    index += char.length
  }
  return index < text.length ? text.slice(0, index) : text
}
export function cutoff(text: string, preferences: SearchPreferences): string {
  return preferences.compression === 'cutoff' ? truncate(text, preferences.cutoffLimit) : text
}
export function processHits(hits: readonly SearchHit[], preferences: SearchPreferences): SearchHit[] {
  const seen = new Set<string>()
  const result: SearchHit[] = []
  for (const hit of hits) {
    let url: string
    try {
      checkBlacklist(hit.url, preferences)
      url = targetUrl(hit.url).href
    } catch {
      continue
    }
    if (seen.has(url)) continue
    seen.add(url)
    result.push({ title: truncate(hit.title || url, 500), url, snippet: cutoff(hit.snippet, preferences) })
    if (result.length >= preferences.maxResults) break
  }
  return result
}
export function validateQuery(value: string): string {
  const query = stringValue(value, 'query').trim()
  if (!query || [...query].length > 4096) throw badRequest('query', 'invalid')
  return query
}
export function validateTarget(value: string): string {
  if (new TextEncoder().encode(stringValue(value, 'url')).length > 8192) throw badRequest('url', 'too_long', 8192)
  return targetUrl(value.trim()).href
}
