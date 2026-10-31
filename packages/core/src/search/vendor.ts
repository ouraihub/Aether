import type { SearchHit, SearchPage } from '@ouraihub/aether-contracts'
import { KitFailure } from '../error.js'
import type { ResolvedSearchConfig } from './execution.js'
import { record, truncate } from './policy.js'

export function at(value: unknown, path: string): unknown {
  let current = value
  for (const key of path.split('/').filter(Boolean)) {
    if (Array.isArray(current)) current = current[Number(key)]
    else if (record(current)) current = current[key]
    else return undefined
  }
  return current
}
function text(value: unknown, ...keys: string[]): string {
  if (!record(value)) return ''
  for (const key of keys) {
    const found: unknown = value[key]
    if (typeof found === 'string' && found.trim()) return found.trim()
    if (Array.isArray(found)) {
      const joined = found
        .filter((part: unknown): part is string => typeof part === 'string')
        .join('\n')
        .trim()
      if (joined) return joined
    }
  }
  return ''
}
function json(body: string): Record<string, unknown> {
  let value: unknown
  try {
    value = JSON.parse(body)
  } catch {
    throw new KitFailure('upstream', 'Search provider returned invalid JSON')
  }
  if (!record(value))
    throw new KitFailure('upstream', 'Search provider returned invalid JSON')
  return value
}
function providerError(
  provider: string,
  payload: Record<string, unknown>,
): void {
  if (
    (provider === 'bocha' &&
      payload.code !== undefined &&
      payload.code !== 200) ||
    (provider === 'querit' &&
      payload.error_code !== undefined &&
      payload.error_code !== 200) ||
    (provider === 'firecrawl' && payload.success === false)
  )
    throw new KitFailure('upstream', 'Search provider returned an error')
}
function collect(items: readonly unknown[]): SearchHit[] {
  const hits: SearchHit[] = []
  for (const item of items) {
    const url = text(item, 'url', 'link')
    if (!url) continue
    hits.push({
      title: text(item, 'title', 'name') || url,
      url,
      snippet: text(
        item,
        'content',
        'text',
        'snippet',
        'description',
        'excerpts',
        'summary',
        'markdown',
      ),
    })
  }
  return hits
}
export function parseSearchResponse(
  provider: string,
  body: string,
): SearchHit[] {
  if (provider === 'exa-mcp') return parseMcp(body)
  const payload = json(body)
  providerError(provider, payload)
  const paths: Readonly<Record<string, readonly string[]>> = {
    zhipu: ['search_result'],
    bocha: ['data/webPages/value'],
    querit: ['results/result'],
    brave: ['web/results'],
    parallel: ['results'],
    jina: ['data', 'results'],
    firecrawl: ['data/web'],
    tavily: ['results'],
    exa: ['results'],
    searxng: ['results'],
  }
  for (const path of paths[provider] ?? []) {
    const items = at(payload, path)
    if (Array.isArray(items)) return collect(items)
  }
  throw new KitFailure(
    'upstream',
    'Search provider returned invalid result shape',
  )
}
export function parseMcp(body: string): SearchHit[] {
  const envelopes: Record<string, unknown>[] = []
  if (body.trimStart().startsWith('{')) envelopes.push(json(body))
  else {
    for (const block of body.replace(/\r\n/g, '\n').split('\n\n')) {
      const data = block
        .split('\n')
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).replace(/^ /, ''))
        .join('\n')
      if (data && data !== '[DONE]') envelopes.push(json(data))
    }
  }
  const chunks: string[] = []
  for (const envelope of envelopes) {
    if (envelope.error !== undefined || at(envelope, 'result/isError') === true)
      throw new KitFailure('upstream', 'Exa MCP returned an error')
    const content = at(envelope, 'result/content')
    if (Array.isArray(content))
      for (const item of content) {
        const value = text(item, 'text')
        if (value) chunks.push(value)
      }
  }
  if (!chunks.length)
    throw new KitFailure('upstream', 'Exa MCP returned invalid result shape')
  return collect(
    chunks
      .join('\n\n')
      .split(/\r?\n(?=Title:)/u)
      .map((chunk) => ({
        title: chunk.match(/^Title:[^\S\r\n]*(.*)$/mu)?.[1] ?? '',
        url: chunk.match(/^URL:[^\S\r\n]*(.*)$/mu)?.[1] ?? '',
        content:
          chunk
            .match(/^(?:Text|Highlights):[^\S\r\n]*([\s\S]*)$/mu)?.[1]
            ?.trim() ?? '',
      })),
  )
}
function headers(
  key: string | undefined,
  kind: 'bearer' | 'api' = 'bearer',
): Record<string, string> {
  return {
    accept: 'application/json',
    'content-type': 'application/json',
    ...(key
      ? kind === 'api'
        ? { 'x-api-key': key }
        : { authorization: 'Bearer ' + key }
      : {}),
  }
}
function basic(username: string, key: string): string {
  const bytes = new TextEncoder().encode(username + ':' + key)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return 'Basic ' + btoa(binary)
}
export function searchRequest(
  config: ResolvedSearchConfig,
  query: string,
  key: string | undefined,
): { url: string; init: RequestInit } {
  let url = config.baseUrl.replace(/\/+$/u, '')
  const count = config.preferences.maxResults
  let init: RequestInit
  const post = (
    payload: unknown,
    kind: 'bearer' | 'api' = 'bearer',
  ): RequestInit => ({
    method: 'POST',
    headers: headers(key, kind),
    body: JSON.stringify(payload),
  })
  switch (config.provider) {
    case 'zhipu':
      init = post({
        search_query: query,
        search_engine: 'search_std',
        search_intent: false,
      })
      break
    case 'tavily':
      url += '/search'
      init = post({ query, max_results: count })
      break
    case 'searxng':
      url +=
        '/search?q=' + encodeURIComponent(query) + '&format=json&language=auto'
      init = {
        headers: {
          accept: 'application/json',
          ...(config.basicAuthUsername
            ? { authorization: basic(config.basicAuthUsername, key ?? '') }
            : {}),
        },
      }
      break
    case 'exa':
      url += '/search'
      init = post({ query, numResults: count, contents: { text: true } }, 'api')
      break
    case 'exa-mcp':
      init = {
        ...post(
          {
            jsonrpc: '2.0',
            id: 1,
            method: 'tools/call',
            params: {
              name: 'web_search_exa',
              arguments: {
                query,
                type: 'auto',
                numResults: count,
                livecrawl: 'fallback',
              },
            },
          },
          'api',
        ),
        headers: {
          ...headers(key, 'api'),
          accept: 'application/json, text/event-stream',
        },
      }
      break
    case 'bocha':
      url += '/v1/web-search'
      init = post({ query, count, exclude: '', summary: true })
      break
    case 'querit':
      url += '/v1/search'
      init = post({ query, count })
      break
    case 'brave':
      url +=
        '/res/v1/web/search?q=' +
        encodeURIComponent(query) +
        '&count=' +
        Math.min(20, count)
      init = {
        headers: {
          accept: 'application/json',
          'x-subscription-token': key ?? '',
        },
      }
      break
    case 'parallel':
      url += '/v1/search'
      init = post(
        {
          objective: query,
          search_queries: [query],
          advanced_settings: { max_results: count },
        },
        'api',
      )
      break
    case 'jina':
      url += '/' + encodeURIComponent(query)
      init = { headers: headers(key) }
      break
    case 'firecrawl':
      url += '/v2/search'
      init = post({
        query,
        limit: count,
        scrapeOptions: { formats: ['markdown'] },
      })
      break
    default:
      throw new KitFailure('bad_request', 'Unsupported search provider', {
        field: 'provider',
        reason: 'unsupported',
      })
  }
  return { url, init }
}
export function fetchRequest(
  config: ResolvedSearchConfig,
  target: string,
  key: string | undefined,
): { url: string; init: RequestInit } {
  const base = config.baseUrl.replace(/\/+$/u, '')
  switch (config.provider) {
    case 'querit':
      return {
        url: base + '/v1/contents',
        init: {
          method: 'POST',
          headers: headers(key),
          body: JSON.stringify({
            urls: [target],
            format: 'markdown',
            extrasMeta: true,
          }),
        },
      }
    case 'jina':
      return {
        url: base + '/' + target,
        init: { headers: { ...headers(key), 'x-retain-images': 'none' } },
      }
    case 'firecrawl':
      return {
        url: base + '/v2/scrape',
        init: {
          method: 'POST',
          headers: headers(key),
          body: JSON.stringify({ url: target, formats: ['markdown'] }),
        },
      }
    case 'fetch':
      return {
        url: target,
        init: { method: 'GET', headers: { accept: 'text/html, text/plain' } },
      }
    default:
      throw new KitFailure('bad_request', 'Unsupported fetch provider', {
        field: 'provider',
        reason: 'unsupported',
      })
  }
}
export function parsePage(
  provider: string,
  body: string,
  target: string,
): SearchPage {
  if (provider === 'fetch') {
    const title =
      body
        .match(/<title\b[^>]*>([\s\S]*?)<\/title>/iu)?.[1]
        ?.replace(/<[^>]*>/gu, ' ')
        .replace(/\s+/gu, ' ')
        .trim() || target
    const visible = body.replace(
      /<!--[\s\S]*?-->|<(script|style|noscript|head|title)\b[^>]*>[\s\S]*?<\/\1\s*>/giu,
      ' ',
    )
    const documentBody =
      visible.match(/<body\b[^>]*>([\s\S]*?)(?:<\/body\s*>|$)/iu)?.[1] ??
      visible
    const content = documentBody
      .replace(/<[^>]*>/gu, ' ')
      .replace(/\s+/gu, ' ')
      .trim()
    if (!content) throw new KitFailure('upstream', 'Page has no content')
    return {
      title: truncate(title, 500),
      url: target,
      content: truncate(content, 24000),
    }
  }
  const payload = json(body)
  providerError(provider, payload)
  let data: unknown
  let metadata: unknown
  if (provider === 'querit') {
    data = at(payload, 'results/0') ?? at(payload, 'data/0')
    metadata = at(data, 'extrasMeta') ?? at(data, 'metadata')
  } else if (provider === 'firecrawl') {
    data = at(payload, 'data')
    metadata = at(data, 'metadata')
  } else {
    data = at(payload, 'data') ?? payload
    metadata = at(data, 'metadata')
  }
  const url =
    text(data, 'url', 'link') || text(metadata, 'sourceURL', 'url') || target
  const title = text(data, 'title') || text(metadata, 'title') || url
  const content = text(data, 'markdown', 'content', 'text')
  if (!content)
    throw new KitFailure('upstream', 'Search provider returned no page content')
  return { title: truncate(title, 500), url, content: truncate(content, 24000) }
}
