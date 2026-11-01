<script lang="ts">
  import ActivityIcon from '@lucide/svelte/icons/activity'
  import ListIcon from '@lucide/svelte/icons/list'
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle'
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import CopyInput from '../CopyInput.svelte'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { getKitContext } from '../context'
  import type { SearchCap, SearchProvider, SearchSettingsDto } from '../ports'

  type Draft = { baseUrl?: string; basicAuthUsername?: string }
  let {
    cap,
    provider,
    settings,
    pending,
    error,
    draft,
    keyDraft,
    check,
    onChoose,
    onOpenKeys,
    onKeyInput,
    onPatchDraft,
    onSave,
  }: {
    cap: SearchCap
    provider: SearchProvider
    settings: SearchSettingsDto
    pending: boolean
    error: string
    draft: Draft
    keyDraft: string
    check?: number | undefined
    onChoose: (provider: string) => void
    onOpenKeys: () => void
    onKeyInput: (value: string) => void
    onPatchDraft: (patch: Draft) => void
    onSave: (check?: boolean) => void
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

<section
  class="flex min-w-0 flex-col gap-3 rounded-kit border p-4"
  aria-labelledby={`search-${cap}`}
>
  <header class="flex flex-wrap items-start justify-between gap-3">
    <div class="min-w-0 flex-1">
      <h2 id={`search-${cap}`} class="text-kit-base font-semibold">{t(`search.${cap}`)}</h2>
      <p class="text-muted-foreground text-kit-sm">
        {provider.description}
        {#if safeUrl(provider.officialWebsite)}<a
            class="text-primary inline-flex"
            href={safeUrl(provider.officialWebsite) ?? ''}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={t('search.website')}
            title={t('search.website')}><ExternalLinkIcon size={13} /></a
          >{/if}
      </p>
    </div>
    <select
      class="bg-background border-input h-kit-control max-w-full rounded-kit-sm border px-2 text-kit-sm"
      aria-label={t(`search.${cap}`)}
      disabled={pending}
      value={settings.defaultProvider}
      onchange={(event) => onChoose(event.currentTarget.value)}
    >
      {#each settings.providers as option (option.id)}<option value={option.id}
          >{option.name}</option
        >{/each}
    </select>
  </header>
  {#if error}<Alert.Root variant="destructive"
      ><Alert.Description>{error}</Alert.Description></Alert.Root
    >{/if}
  <Field.Group class="gap-3 border-t pt-3">
    {#if provider.keyMode !== 'none' || provider.basicAuth}
      <Field.Field data-disabled={pending}>
        <div class="flex flex-wrap items-baseline gap-2">
          <Field.Label for={`search-key-${cap}`}
            >{provider.basicAuth ? t('search.password') : t('search.apiKey')}</Field.Label
          >
          {#if safeUrl(provider.apiKeyWebsite)}<a
              class="text-primary text-kit-sm hover:underline"
              href={safeUrl(provider.apiKeyWebsite) ?? ''}
              target="_blank"
              rel="noopener noreferrer">{t('search.apiKeyWebsite')}</a
            >{/if}
        </div>
        <div class="flex min-w-0 items-center gap-2">
          <Input
            id={`search-key-${cap}`}
            class="min-w-0 flex-1"
            type="password"
            autocomplete="off"
            spellcheck={false}
            maxlength={4096}
            disabled={pending}
            placeholder={t('search.newKey')}
            value={keyDraft}
            oninput={(event) => onKeyInput(event.currentTarget.value)}
          />
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            disabled={pending}
            aria-label={t('search.manageKeys')}
            title={t('search.manageKeys')}
            onclick={onOpenKeys}><ListIcon /></Button
          >
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            disabled={pending}
            aria-label={t('search.check')}
            title={t('search.check')}
            onclick={() => onSave(true)}
          >
            {#if pending}<LoaderCircleIcon class="animate-spin" />{:else}<ActivityIcon />{/if}
          </Button>
        </div>
        <Field.Description
          >{provider.keys.length
            ? provider.keys.map((item) => item.masked).join(' · ')
            : t('search.keyNotSet')}</Field.Description
        >
      </Field.Field>
    {/if}
    {#if provider.defaultBaseUrl !== undefined}
      <Field.Field data-disabled={pending}>
        <Field.Label for={`search-url-${cap}`}>{t('search.baseUrl')}</Field.Label>
        <div class="flex min-w-0 items-center gap-2">
          <CopyInput
            id={`search-url-${cap}`}
            class="min-w-0 flex-1"
            disabled={pending}
            value={draft.baseUrl ?? provider.baseUrl ?? provider.defaultBaseUrl}
            oninput={(event) => onPatchDraft({ baseUrl: event.currentTarget.value })}
          />
          {#if provider.keyMode === 'none' && !provider.basicAuth}<Button
              type="button"
              variant="outline"
              size="icon-sm"
              disabled={pending}
              aria-label={t('search.check')}
              title={t('search.check')}
              onclick={() => onSave(true)}><ActivityIcon /></Button
            >{/if}
        </div>
      </Field.Field>
    {/if}
    {#if provider.basicAuth}
      <Field.Field
        ><Field.Label for={`search-user-${cap}`}>{t('search.username')}</Field.Label><Input
          id={`search-user-${cap}`}
          disabled={pending}
          value={draft.basicAuthUsername ?? provider.basicAuthUsername ?? ''}
          oninput={(event) => onPatchDraft({ basicAuthUsername: event.currentTarget.value })}
        /></Field.Field
      >
    {/if}
  </Field.Group>
  <div class="flex flex-wrap items-center gap-2">
    {#if provider.defaultBaseUrl !== undefined || provider.keyMode !== 'none'}<Button
        type="button"
        size="sm"
        disabled={pending}
        onclick={() => onSave()}>{t('search.save')}</Button
      >{/if}
    {#if pending}<span class="text-muted-foreground text-kit-sm" role="status"
        >{t('search.checking')}</span
      >
    {:else if check !== undefined}<span class="text-muted-foreground text-kit-sm" role="status"
        >{t('search.resultCount', { count: check })}</span
      >{/if}
  </div>
</section>
