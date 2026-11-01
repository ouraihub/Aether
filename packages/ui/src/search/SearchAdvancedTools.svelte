<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { getKitContext } from '../context'
  import type { SearchPage, SearchPreferences, SearchResult } from '../ports'
  let {
    preferences = $bindable(),
    blacklist = $bindable(),
    query = $bindable(),
    pageUrl = $bindable(),
    preferencesPending,
    preferencesError,
    result,
    page,
    queryPending,
    pagePending,
    queryError,
    pageError,
    onSavePreferences,
    onSearch,
    onReadPage,
  }: {
    preferences: SearchPreferences
    blacklist: string
    query: string
    pageUrl: string
    preferencesPending: boolean
    preferencesError: string
    result: SearchResult | null
    page: SearchPage | null
    queryPending: boolean
    pagePending: boolean
    queryError: string
    pageError: string
    onSavePreferences: () => void
    onSearch: () => void
    onReadPage: () => void
  } = $props()

  const { i18n } = getKitContext()
  const { t } = i18n
  const safeUrl = (raw?: string) => {
    if (!raw) return null
    try {
      const url = new URL(raw)
      return ['https:', 'http:'].includes(url.protocol) ? url.href : null
    } catch {
      return null
    }
  }
</script>

<section class="rounded-kit border p-4">
  <details>
    <summary class="cursor-pointer text-kit-base font-semibold">{t('search.advanced')}</summary>
    <Field.Group class="mt-3 gap-3">
      <Field.Field orientation="horizontal"
        ><Field.Label for="search-max-results">{t('search.maxResults')}</Field.Label><Input
          id="search-max-results"
          class="ml-auto w-20"
          type="number"
          min="1"
          max="50"
          disabled={preferencesPending}
          value={preferences.maxResults}
          oninput={(event) => (preferences.maxResults = +event.currentTarget.value)}
        /></Field.Field
      >
      <Field.Field orientation="horizontal"
        ><Field.Label for="search-compression">{t('search.compression')}</Field.Label><select
          id="search-compression"
          class="bg-background border-input h-kit-control ml-auto rounded-kit-sm border px-2 text-kit-sm"
          disabled={preferencesPending}
          value={preferences.compression}
          onchange={(event) =>
            (preferences.compression = event.currentTarget
              .value as SearchPreferences['compression'])}
          ><option value="none">{t('search.compressionNone')}</option><option value="cutoff"
            >{t('search.compressionCutoff')}</option
          ></select
        ></Field.Field
      >
      {#if preferences.compression === 'cutoff'}<Field.Field orientation="horizontal"
          ><Field.Label for="search-cutoff">{t('search.cutoffLimit')}</Field.Label><Input
            id="search-cutoff"
            class="ml-auto w-24"
            type="number"
            min="100"
            max="20000"
            disabled={preferencesPending}
            value={preferences.cutoffLimit}
            oninput={(event) => (preferences.cutoffLimit = +event.currentTarget.value)}
          /></Field.Field
        >{/if}
      <Field.Field
        ><Field.Label for="search-blacklist">{t('search.blacklist')}</Field.Label><Textarea
          id="search-blacklist"
          rows={3}
          disabled={preferencesPending}
          placeholder="example.com\n*://*.example.org/*"
          value={blacklist}
          oninput={(event) => (blacklist = event.currentTarget.value)}
        /></Field.Field
      >
    </Field.Group>
  </details>
  <Field.Field class="mt-3 border-t pt-3"
    ><label class="flex items-center gap-2 text-kit-sm"
      ><input
        type="checkbox"
        disabled={preferencesPending}
        checked={preferences.modelToolsPreferred}
        onchange={(event) => (preferences.modelToolsPreferred = event.currentTarget.checked)}
      />{t('search.nativePreferred')}</label
    ><Field.Description>{t('search.nativePrivacy')}</Field.Description></Field.Field
  >
  {#if preferencesError}<p role="alert" class="text-destructive text-kit-sm">
      {preferencesError}
    </p>{/if}
  <Button
    class="mt-3"
    type="button"
    size="sm"
    disabled={preferencesPending}
    onclick={onSavePreferences}>{t('search.save')}</Button
  >
</section>
<section class="flex min-w-0 flex-col gap-3 rounded-kit border p-4" aria-label={t('search.tools')}>
  <Field.Field
    ><Field.Label for="search-query">{t('search.query')}</Field.Label>
    <div class="flex min-w-0 gap-2">
      <Input
        id="search-query"
        class="min-w-0 flex-1"
        disabled={queryPending}
        value={query}
        oninput={(event) => (query = event.currentTarget.value)}
      /><Button type="button" disabled={queryPending || !query.trim()} onclick={onSearch}
        >{t('search.runSearch')}</Button
      >
    </div></Field.Field
  >
  {#if queryError}<p class="text-destructive text-kit-sm" role="alert">{queryError}</p>{/if}
  {#if result}<p class="text-muted-foreground text-kit-sm">
      {t('search.resultCount', { count: result.hits.length })}
    </p>
    {#each result.hits as hit (hit.url)}<article class="min-w-0 text-kit-sm">
        <a
          class="text-primary break-words hover:underline"
          href={safeUrl(hit.url) ?? ''}
          target="_blank"
          rel="noopener noreferrer">{hit.title}</a
        >
        <p class="whitespace-pre-wrap break-words">{hit.snippet}</p>
      </article>{/each}{/if}
  <Field.Field
    ><Field.Label for="search-page-url">{t('search.pageUrl')}</Field.Label>
    <div class="flex min-w-0 gap-2">
      <Input
        id="search-page-url"
        class="min-w-0 flex-1"
        disabled={pagePending}
        value={pageUrl}
        oninput={(event) => (pageUrl = event.currentTarget.value)}
      /><Button type="button" disabled={pagePending || !pageUrl.trim()} onclick={onReadPage}
        >{t('search.read')}</Button
      >
    </div></Field.Field
  >
  {#if pageError}<p class="text-destructive text-kit-sm" role="alert">{pageError}</p>{/if}
  {#if page}<h3 class="text-kit-base font-medium">{page.title}</h3>
    <a
      class="text-primary break-words text-kit-sm"
      href={safeUrl(page.url) ?? ''}
      target="_blank"
      rel="noopener noreferrer">{page.url}</a
    >
    <pre
      class="max-h-96 overflow-auto whitespace-pre-wrap break-words text-kit-sm">{page.content}</pre>{/if}
</section>
