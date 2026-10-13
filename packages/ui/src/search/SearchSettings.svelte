<script lang="ts">
  import { onMount } from 'svelte'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import {
    isKitClientError,
    type SearchCap,
    type SearchPage,
    type SearchProvider,
    type SearchSettingsDto,
  } from '../ports'

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const port = ports.search
  const capabilities: SearchCap[] = ['search', 'fetch']
  type Draft = { baseUrl?: string; apiKey?: string; basicAuthUsername?: string }
  type Check = { status: 'running' } | { status: 'ok'; count: number } | { status: 'failed'; message: string }

  let settings = $state<Partial<Record<SearchCap, SearchSettingsDto>>>({})
  let selected = $state<Partial<Record<SearchCap, string>>>({})
  let drafts = $state<Record<string, Draft>>({})
  let checks = $state<Record<string, Check>>({})
  let busy = $state<Record<string, boolean>>({})
  let errors = $state<Record<string, string>>({})
  let revealed = $state<Record<string, string>>({})
  let revealPassword = $state<Record<string, string>>({})
  let maxResults = $state('')
  let query = $state('')
  let queryResult = $state<{
    query: string
    provider: string
    hits: { title: string; url: string; snippet: string }[]
  } | null>(null)
  let pageUrl = $state('')
  let page = $state<SearchPage | null>(null)
  let pageError = $state('')
  let editing = $state<Record<string, boolean>>({})
  let searchBusy = $state(false)
  let readBusy = $state(false)
  let abort = new AbortController()
  let loading = $state(true)

  const keyOf = (capability: SearchCap, provider: string) => `${capability}:${provider}`
  const textError = (error: unknown) => (isKitClientError(error) ? t(`errors.${error.code}`) : String(error))
  const safeUrl = (value?: string): string | null => {
    if (!value) return null
    try {
      const protocol = new URL(value).protocol
      return protocol === 'http:' || protocol === 'https:' ? value : null
    } catch {
      return null
    }
  }
  const providers = (capability: SearchCap) => settings[capability]?.providers ?? []
  const activeProvider = (capability: SearchCap) => {
    const current = settings[capability]
    return providers(capability).find((provider) => provider.id === (selected[capability] ?? current?.defaultProvider))
  }
  const draftOf = (capability: SearchCap, provider: string) => drafts[keyOf(capability, provider)] ?? {}

  function error(capability: SearchCap, value: unknown) {
    errors = { ...errors, [capability]: textError(value) }
  }
  function apply(next: SearchSettingsDto) {
    settings = { ...settings, [next.capability]: next }
    selected = {
      ...selected,
      [next.capability]: next.providers.find((p) => p.id === next.defaultProvider)?.id ?? next.defaultProvider,
    }
    if (next.capability === 'search') maxResults = String(next.maxResults ?? 5)
  }
  function patchDraft(capability: SearchCap, provider: string, patch: Draft) {
    const key = keyOf(capability, provider)
    drafts = { ...drafts, [key]: { ...drafts[key], ...patch } }
  }
  async function load() {
    if (!port || loading === false) return
    loading = true
    const controller = new AbortController()
    abort.abort()
    abort = controller
    try {
      await Promise.all(
        capabilities.map(async (capability) => {
          const result = await port.settings(capability, { signal: controller.signal })
          if (!controller.signal.aborted) apply(result)
        }),
      )
    } catch (e) {
      if (!controller.signal.aborted) notify.toast('error', textError(e))
    } finally {
      if (!controller.signal.aborted) loading = false
    }
  }
  async function choose(capability: SearchCap, provider: string) {
    const current = settings[capability]
    if (!port || !current || provider === current.defaultProvider || busy[capability]) return
    busy = { ...busy, [capability]: true }
    errors = { ...errors, [capability]: '' }
    try {
      apply(
        await port.setDefault(capability, provider, capability === 'search' ? Number(maxResults) || 5 : undefined, {
          signal: abort.signal,
        }),
      )
      revealed = Object.fromEntries(Object.entries(revealed).filter(([name]) => !name.startsWith(`${capability}:`)))
      revealPassword = Object.fromEntries(
        Object.entries(revealPassword).filter(([name]) => !name.startsWith(`${capability}:`)),
      )
    } catch (e) {
      error(capability, e)
      selected = { ...selected, [capability]: current.defaultProvider }
    } finally {
      busy = { ...busy, [capability]: false }
    }
  }
  async function save(capability: SearchCap, provider: SearchProvider) {
    if (!port || busy[capability]) return
    const draft = draftOf(capability, provider.id)
    busy = { ...busy, [capability]: true }
    try {
      apply(
        await port.save(
          {
            capability,
            provider: provider.id,
            ...(provider.defaultBaseUrl !== undefined
              ? { baseUrl: (draft.baseUrl ?? provider.baseUrl ?? '').trim() }
              : {}),
            ...(draft.apiKey !== undefined ? { apiKey: draft.apiKey } : {}),
            ...(provider.basicAuth
              ? { basicAuthUsername: (draft.basicAuthUsername ?? provider.basicAuthUsername ?? '').trim() }
              : {}),
          },
          { signal: abort.signal },
        ),
      )
      const key = keyOf(capability, provider.id)
      drafts = Object.fromEntries(Object.entries(drafts).filter(([name]) => name !== key))
      checks = Object.fromEntries(Object.entries(checks).filter(([name]) => name !== key))
      editing = { ...editing, [key]: false }
      revealed = Object.fromEntries(Object.entries(revealed).filter(([name]) => name !== key))
      notify.toast('success', t('search.saved'))
    } catch (e) {
      error(capability, e)
    } finally {
      busy = { ...busy, [capability]: false }
    }
  }
  async function setCount() {
    if (!port || !settings.search) return
    const value = Math.min(50, Math.max(1, Number.isFinite(Number(maxResults)) ? Math.round(Number(maxResults)) : 5))
    try {
      apply(await port.setDefault('search', settings.search.defaultProvider, value, { signal: abort.signal }))
    } catch (e) {
      error('search', e)
    }
  }
  async function check(capability: SearchCap, provider: SearchProvider) {
    if (!port) return
    const key = keyOf(capability, provider.id)
    if (busy[key]) return
    busy = { ...busy, [key]: true }
    checks = { ...checks, [key]: { status: 'running' } }
    try {
      checks = {
        ...checks,
        [key]: { status: 'ok', count: (await port.check(capability, provider.id, { signal: abort.signal })).count },
      }
    } catch (e) {
      if (!abort.signal.aborted) checks = { ...checks, [key]: { status: 'failed', message: textError(e) } }
    } finally {
      busy = { ...busy, [key]: false }
    }
  }
  async function reveal(capability: SearchCap, provider: SearchProvider) {
    if (!port) return
    const key = keyOf(capability, provider.id)
    if (busy[key]) return
    busy = { ...busy, [key]: true }
    try {
      revealed = {
        ...revealed,
        [key]: await port.reveal(capability, provider.id, port.revealNeedsPassword ? revealPassword[key] : undefined, {
          signal: abort.signal,
        }),
      }
    } catch (e) {
      if (!abort.signal.aborted) error(capability, e)
    } finally {
      busy = { ...busy, [key]: false }
    }
  }
  async function search() {
    if (!port || !query.trim() || searchBusy) return
    searchBusy = true
    try {
      queryResult = await port.search(query.trim(), { signal: abort.signal })
    } catch (e) {
      queryResult = null
      notify.toast('error', textError(e))
    } finally {
      searchBusy = false
    }
  }
  async function readUrl() {
    if (!port || !pageUrl.trim() || readBusy) return
    readBusy = true
    pageError = ''
    page = null
    try {
      page = await port.fetch(pageUrl.trim(), { signal: abort.signal })
    } catch (e) {
      pageError = textError(e)
    } finally {
      readBusy = false
    }
  }

  onMount(() => {
    if (port) void load()
    return () => abort.abort()
  })
</script>

{#if port}
  {#if loading}
    <p class="text-muted-foreground text-kit-sm">{t('search.loading')}</p>
  {:else}
    <div class="flex flex-col gap-6">
      {#each capabilities as capability (capability)}
        {@const current = settings[capability]}
        {@const provider = activeProvider(capability)}
        {#if current && provider}
          <section class="flex flex-col gap-4 rounded-kit border p-4" aria-labelledby={`search-${capability}`}>
            <header class="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h2 id={`search-${capability}`} class="text-kit-base font-semibold">{t(`search.${capability}`)}</h2>
                <p class="text-muted-foreground text-kit-sm">{t(`search.${capability}Hint`)}</p>
              </div>
              <label class="flex items-center gap-2 text-kit-sm">
                <span>{t('search.defaultProvider')}</span>
                <select
                  class="bg-background border-input h-kit-control rounded-kit-sm border px-2"
                  value={selected[capability] ?? current.defaultProvider}
                  disabled={busy[capability]}
                  onchange={(event) => void choose(capability, event.currentTarget.value)}
                >
                  {#each current.providers as option (option.id)}<option value={option.id}>{option.name}</option>{/each}
                </select>
              </label>
            </header>
            {#if errors[capability]}<Alert.Root variant="destructive"
                ><Alert.Description>{errors[capability]}</Alert.Description></Alert.Root
              >{/if}
            <div class="flex flex-col gap-3">
              <div>
                <h3 class="font-medium">{provider.name}</h3>
                <p class="text-muted-foreground text-kit-sm">{provider.description}</p>
                <div class="flex gap-3 text-kit-sm">
                  {#if safeUrl(provider.officialWebsite)}<a
                      class="text-primary underline"
                      href={safeUrl(provider.officialWebsite) ?? ''}
                      target="_blank"
                      rel="noopener noreferrer">{t('search.website')}</a
                    >{/if}
                  {#if safeUrl(provider.apiKeyWebsite)}<a
                      class="text-primary underline"
                      href={safeUrl(provider.apiKeyWebsite) ?? ''}
                      target="_blank"
                      rel="noopener noreferrer">{t('search.apiKeyWebsite')}</a
                    >{/if}
                </div>
              </div>
              {#if provider.defaultBaseUrl !== undefined}
                <Field.Field
                  ><Field.Label for={`search-url-${capability}`}>{t('search.baseUrl')}</Field.Label><Input
                    id={`search-url-${capability}`}
                    value={draftOf(capability, provider.id).baseUrl ?? provider.baseUrl ?? provider.defaultBaseUrl}
                    oninput={(e) => patchDraft(capability, provider.id, { baseUrl: e.currentTarget.value })}
                  /></Field.Field
                >
              {/if}
              {#if provider.basicAuth}
                <Field.Field
                  ><Field.Label for={`search-user-${capability}`}>{t('search.username')}</Field.Label><Input
                    id={`search-user-${capability}`}
                    value={draftOf(capability, provider.id).basicAuthUsername ?? provider.basicAuthUsername ?? ''}
                    oninput={(e) => patchDraft(capability, provider.id, { basicAuthUsername: e.currentTarget.value })}
                  /></Field.Field
                >
              {/if}
              {#if provider.keyMode !== 'none' || provider.basicAuth}
                <Field.Field>
                  <Field.Label for={`search-key-${capability}`}
                    >{provider.basicAuth && provider.keyMode === 'none'
                      ? t('search.password')
                      : t('search.apiKey')}</Field.Label
                  >
                  {#if editing[keyOf(capability, provider.id)]}<Input
                      id={`search-key-${capability}`}
                      type="password"
                      value={draftOf(capability, provider.id).apiKey ?? ''}
                      oninput={(e) => {
                        const key = keyOf(capability, provider.id)
                        const rest = { ...drafts[key] }
                        delete rest.apiKey
                        const value = e.currentTarget.value
                        drafts = { ...drafts, [key]: value ? { ...rest, apiKey: value } : rest }
                      }}
                    />
                  {:else if draftOf(capability, provider.id).apiKey === ''}<p class="text-muted-foreground text-kit-sm">
                      {t('search.keyCleared')}
                    </p>
                  {:else}<p id={`search-key-${capability}`} class="max-w-full truncate font-mono text-kit-sm">
                      {provider.apiKeyMasked ?? t('search.keyNotSet')}
                    </p>{/if}
                  <div class="flex flex-wrap gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onclick={() => {
                        patchDraft(capability, provider.id, { apiKey: '' })
                        editing = { ...editing, [keyOf(capability, provider.id)]: false }
                      }}>{t('search.clearKey')}</Button
                    >
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onclick={() => {
                        const key = keyOf(capability, provider.id)
                        const rest = { ...drafts[key] }
                        delete rest.apiKey
                        drafts = { ...drafts, [key]: rest }
                        editing = { ...editing, [key]: true }
                      }}>{t('search.replaceKey')}</Button
                    >
                    {#if editing[keyOf(capability, provider.id)] || draftOf(capability, provider.id).apiKey !== undefined}
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onclick={() => {
                          const key = keyOf(capability, provider.id)
                          const rest = { ...drafts[key] }
                          delete rest.apiKey
                          drafts = { ...drafts, [key]: rest }
                          editing = { ...editing, [key]: false }
                        }}>{t('providers.key_undo')}</Button
                      >
                    {/if}
                    {#if port.revealNeedsPassword}<Input
                        type="password"
                        aria-label={t('search.currentPassword')}
                        placeholder={t('search.currentPassword')}
                        value={revealPassword[keyOf(capability, provider.id)] ?? ''}
                        oninput={(e) =>
                          (revealPassword = {
                            ...revealPassword,
                            [keyOf(capability, provider.id)]: e.currentTarget.value,
                          })}
                      />{/if}
                    {#if provider.apiKeyMasked}<Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onclick={() => void reveal(capability, provider)}>{t('search.reveal')}</Button
                      >{/if}
                  </div>
                  {#if revealed[keyOf(capability, provider.id)]}<p
                      class="max-w-full truncate font-mono text-kit-sm"
                      role="status"
                    >
                      {revealed[keyOf(capability, provider.id)]}
                    </p>{/if}
                </Field.Field>
              {/if}
              <div class="flex flex-wrap items-center gap-2">
                <Button type="button" disabled={busy[capability]} onclick={() => void save(capability, provider)}
                  >{t('search.save')}</Button
                >
                <Button type="button" variant="outline" onclick={() => void check(capability, provider)}
                  >{t('search.check')}</Button
                >
                {#if checks[keyOf(capability, provider.id)]?.status === 'running'}<span
                    class="text-muted-foreground text-kit-sm">{t('search.checking')}</span
                  >{/if}
                {#if checks[keyOf(capability, provider.id)]?.status === 'ok'}<span
                    class="text-muted-foreground text-kit-sm"
                    >{t('search.resultCount', {
                      count: (checks[keyOf(capability, provider.id)] as Extract<Check, { status: 'ok' }>).count,
                    })}</span
                  >{/if}
                {#if checks[keyOf(capability, provider.id)]?.status === 'failed'}<span
                    class="text-destructive text-kit-sm"
                    >{(checks[keyOf(capability, provider.id)] as Extract<Check, { status: 'failed' }>).message}</span
                  >{/if}
              </div>
            </div>
            {#if capability === 'search'}
              <Field.Field class="max-w-xs"
                ><Field.Label for="search-max-results">{t('search.maxResults')}</Field.Label><Input
                  id="search-max-results"
                  type="number"
                  min="1"
                  max="50"
                  value={maxResults}
                  oninput={(e) => (maxResults = e.currentTarget.value)}
                  onblur={() => void setCount()}
                /></Field.Field
              >
            {/if}
          </section>
        {:else}
          <Empty.Root><Empty.Description>{t('search.unavailable')}</Empty.Description></Empty.Root>
        {/if}
      {/each}
      <section class="flex flex-col gap-4 rounded-kit border p-4" aria-labelledby="search-tools">
        <h2 id="search-tools" class="text-kit-base font-semibold">{t('search.tools')}</h2>
        <Field.Field
          ><Field.Label for="search-query">{t('search.query')}</Field.Label>
          <div class="flex gap-2">
            <Input id="search-query" bind:value={query} /><Button
              type="button"
              disabled={searchBusy}
              onclick={() => void search()}>{t('search.runSearch')}</Button
            >
          </div></Field.Field
        >
        {#if queryResult}<div class="flex flex-col gap-2" aria-live="polite">
            <p class="text-muted-foreground text-kit-sm">
              {t('search.resultCount', { count: queryResult.hits.length })}
            </p>
            {#each queryResult.hits as hit (hit.url)}<article class="border-b pb-2">
                <a class="text-primary underline" href={hit.url} target="_blank" rel="noreferrer">{hit.title}</a>
                <p class="text-kit-sm">{hit.snippet}</p>
                <p class="text-muted-foreground truncate text-kit-sm">{hit.url}</p>
              </article>{/each}
          </div>{/if}
        <Field.Field
          ><Field.Label for="search-page-url">{t('search.pageUrl')}</Field.Label>
          <div class="flex gap-2">
            <Input id="search-page-url" bind:value={pageUrl} /><Button
              type="button"
              disabled={readBusy}
              onclick={() => void readUrl()}>{t('search.read')}</Button
            >
          </div></Field.Field
        >
        {#if pageError}<p class="text-destructive text-kit-sm" role="alert">{pageError}</p>{/if}
        {#if page}<article class="flex flex-col gap-1" aria-live="polite">
            <h3 class="font-medium">{page.title}</h3>
            <a class="text-primary truncate text-kit-sm underline" href={page.url} target="_blank" rel="noreferrer"
              >{page.url}</a
            >
            <pre
              class="bg-muted max-h-96 overflow-auto rounded-kit p-3 text-kit-sm whitespace-pre-wrap">{page.content}</pre>
          </article>{/if}
      </section>
    </div>
  {/if}
{:else}
  <Empty.Root><Empty.Description>{t('search.unavailable')}</Empty.Description></Empty.Root>
{/if}
