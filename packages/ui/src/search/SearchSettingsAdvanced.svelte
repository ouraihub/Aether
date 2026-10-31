<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { getKitContext } from '../context'
  import type { SearchPage, SearchPreferences } from '../ports'

  let {
    preferences = $bindable<SearchPreferences>(),
    blacklist = $bindable(''),
    query = $bindable(''),
    pendingPreferences,
    pendingQuery,
    pageUrl = $bindable(''),
    pendingPage,
    preferencesError,
    queryError,
    pageError,
    page,
    safeUrl,
    onsavepreferences,
    onsearch,
    onreadpage,
  }: {
    preferences: SearchPreferences
    blacklist: string
    query: string
    pendingPreferences: boolean
    pendingQuery: boolean
    pendingPage: boolean
    preferencesError: string
    queryError: string
    pageError: string
    pageUrl: string
    page: SearchPage | null
    safeUrl: (raw?: string) => string | null
    onsavepreferences: () => void
    onsearch: () => void
    onreadpage: () => void
  } = $props()
  const { t } = getKitContext().i18n
</script>

<section class="rounded-kit border p-4">
  <details>
    <summary class="cursor-pointer text-kit-base font-semibold">
      {t('search.advanced')}
    </summary>
    <Field.Group class="mt-3 gap-3">
      <Field.Field orientation="horizontal">
        <Field.Label for="search-max-results">
          {t('search.maxResults')}
        </Field.Label>
        <Input
          id="search-max-results"
          class="ml-auto w-20"
          type="number"
          min="1"
          max="50"
          disabled={pendingPreferences}
          bind:value={preferences.maxResults}
        />
      </Field.Field>
      <Field.Field orientation="horizontal">
        <Field.Label for="search-compression">
          {t('search.compression')}
        </Field.Label>
        <select
          id="search-compression"
          class="bg-background border-input h-kit-control ml-auto rounded-kit-sm
            border px-2 text-kit-sm"
          disabled={pendingPreferences}
          bind:value={preferences.compression}
        >
          <option value="none">{t('search.compressionNone')}</option>
          <option value="cutoff">{t('search.compressionCutoff')}</option>
        </select>
      </Field.Field>
      {#if preferences.compression === 'cutoff'}
        <Field.Field orientation="horizontal">
          <Field.Label for="search-cutoff">
            {t('search.cutoffLimit')}
          </Field.Label>
          <Input
            id="search-cutoff"
            class="ml-auto w-24"
            type="number"
            min="100"
            max="20000"
            disabled={pendingPreferences}
            bind:value={preferences.cutoffLimit}
          />
        </Field.Field>
      {/if}
      <Field.Field>
        <Field.Label for="search-blacklist">
          {t('search.blacklist')}
        </Field.Label>
        <Textarea
          id="search-blacklist"
          rows={3}
          disabled={pendingPreferences}
          placeholder="example.com\n*://*.example.org/*"
          bind:value={blacklist}
        />
      </Field.Field>
    </Field.Group>
  </details>
  <Field.Field class="mt-3 border-t pt-3">
    <label class="flex items-center gap-2 text-kit-sm">
      <input
        type="checkbox"
        disabled={pendingPreferences}
        bind:checked={preferences.modelToolsPreferred}
      />{t('search.nativePreferred')}
    </label>
    <Field.Description>{t('search.nativePrivacy')}</Field.Description>
  </Field.Field>
  {#if preferencesError}
    <p role="alert" class="text-destructive text-kit-sm">
      {preferencesError}
    </p>
  {/if}
  <Button
    class="mt-3"
    type="button"
    size="sm"
    disabled={pendingPreferences}
    onclick={onsavepreferences}
  >{t('search.save')}</Button>
</section>

<section
  class="flex min-w-0 flex-col gap-3 rounded-kit border p-4"
  aria-label={t('search.tools')}
>
  <Field.Field>
    <Field.Label for="search-query">{t('search.query')}</Field.Label>
    <div class="flex min-w-0 gap-2">
      <Input
        id="search-query"
        class="min-w-0 flex-1"
        disabled={pendingQuery}
        bind:value={query}
      />
      <Button
        type="button"
        disabled={pendingQuery || !query.trim()}
        onclick={onsearch}
      >{t('search.runSearch')}</Button>
    </div>
  </Field.Field>
  {#if queryError}
    <p class="text-destructive text-kit-sm" role="alert">
      {queryError}
    </p>
  {/if}
  <Field.Field>
    <Field.Label for="search-page-url">{t('search.pageUrl')}</Field.Label>
    <div class="flex min-w-0 gap-2">
      <Input
        id="search-page-url"
        class="min-w-0 flex-1"
        disabled={pendingPage}
        bind:value={pageUrl}
      />
      <Button
        type="button"
        disabled={pendingPage || !pageUrl.trim()}
        onclick={onreadpage}
      >{t('search.read')}</Button>
    </div>
  </Field.Field>
  {#if pageError}
    <p class="text-destructive text-kit-sm" role="alert">
      {pageError}
    </p>
  {/if}
  {#if page}
    <h3 class="text-kit-base font-medium">{page.title}</h3>
    <a
      class="text-primary break-words text-kit-sm"
      href={safeUrl(page.url) ?? ''}
      target="_blank"
      rel="noopener noreferrer"
    >{page.url}</a>
    <pre
      class="max-h-96 overflow-auto whitespace-pre-wrap break-words text-kit-sm"
    >{page.content}</pre>
  {/if}
</section>
