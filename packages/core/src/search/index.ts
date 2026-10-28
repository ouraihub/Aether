import type { SearchPage, SearchResult } from '@ouraihub/aether-contracts'
import { KitFailure } from '../error.js'
import type { OutboundHttp } from '../ports.js'
import { execute, type CheckTarget, type ResolvedSearchConfig } from './execution.js'
import { checkBlacklist, cutoff, processHits, validatePreferences, validateQuery, validateTarget } from './policy.js'
import { fetchRequest, parsePage, parseSearchResponse, searchRequest } from './vendor.js'

export type { CheckTarget, ResolvedSearchConfig } from './execution.js'
export {
  PROVIDERS,
  DEFAULT_PROVIDERS,
  defaultPreferences,
  providerFor,
  validatePreferences,
  validateQuery,
  validateTarget,
  checkBlacklist,
  cutoff,
  processHits,
} from './policy.js'

export async function search(
  config: ResolvedSearchConfig,
  rawQuery: string,
  outbound: OutboundHttp,
  signal?: AbortSignal,
): Promise<SearchResult> {
  const query = validateQuery(rawQuery)
  const preferences = validatePreferences(config.preferences)
  const snapshot = { ...config, preferences }
  return execute('search', snapshot, outbound, signal, async (key, execution) => {
    const { url, init } = searchRequest(snapshot, query, key)
    const response = await execution.request(url, init)
    if (response.status >= 300 && response.status < 400) {
      await response.body?.cancel().catch(() => {})
      throw new KitFailure('upstream', 'Search provider redirects are forbidden', { status: response.status })
    }
    const body = await execution.body(response)
    return {
      query,
      provider: config.provider,
      hits: processHits(parseSearchResponse(config.provider, body), preferences),
    }
  })
}
export async function fetch(
  config: ResolvedSearchConfig,
  rawUrl: string,
  outbound: OutboundHttp,
  checkTarget: CheckTarget,
  signal?: AbortSignal,
): Promise<SearchPage> {
  const target = validateTarget(rawUrl)
  const preferences = validatePreferences(config.preferences)
  checkBlacklist(target, preferences)
  const snapshot = { ...config, preferences }
  return execute('fetch', snapshot, outbound, signal, async (key, execution) => {
    // Third-party proxy DNS cannot be pinned; this is a local all-address preflight only.
    if (config.provider !== 'fetch') await execution.wait(checkTarget(target, execution.signal))
    let { url, init } = fetchRequest(snapshot, target, key)
    for (let redirects = 0; ; redirects++) {
      if (config.provider === 'fetch') {
        checkBlacklist(url, preferences)
        await execution.wait(checkTarget(url, execution.signal))
      }
      const response = await execution.request(url, init)
      if (response.status >= 300 && response.status < 400) {
        const location = response.headers.get('location')
        await response.body?.cancel().catch(() => {})
        if (config.provider !== 'fetch' || !location || redirects >= 5)
          throw new KitFailure('upstream', 'Fetch redirect rejected', { status: response.status })
        let next: string
        try {
          next = new URL(location, url).href
        } catch {
          throw new KitFailure('upstream', 'Invalid fetch redirect')
        }
        url = validateTarget(next)
        // Credentials, headers, and methods are rebuilt from the direct-fetch contract.
        init = { method: 'GET', headers: { accept: 'text/html, text/plain' } }
        continue
      }
      const page = parsePage(
        config.provider,
        await execution.body(response),
        config.provider === 'fetch' ? url : target,
      )
      checkBlacklist(page.url, preferences)
      return { ...page, content: cutoff(page.content, preferences) }
    }
  })
}
