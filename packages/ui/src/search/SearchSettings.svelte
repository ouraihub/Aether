<script lang="ts">
  import { onMount } from 'svelte'
  import ActivityIcon from '@lucide/svelte/icons/activity'
  import ListIcon from '@lucide/svelte/icons/list'
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle'
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import CopyInput from '../CopyInput.svelte'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { getKitContext } from '../context'
  import { isKitClientError } from '../ports'
  import { createClientSession } from '../session'
  import { saveSearchSettings } from './settings-flow'
  import type {
    SearchCap,
    SearchKey,
    SearchPage,
    SearchPreferences,
    SearchProvider,
    SearchResult,
    SearchSettingsDto,
  } from '../ports'

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const port = ports.search
  const caps: SearchCap[] = ['search', 'fetch']
  type Draft = { baseUrl?: string; basicAuthUsername?: string }
  let settings = $state<Partial<Record<SearchCap, SearchSettingsDto>>>({})
  let drafts = $state<Record<string, Draft>>({})
  let keyDrafts = $state<Record<string, string>>({})
  const draftRevisions: Record<string, Partial<Record<keyof Draft, number>>> = {}
  const keyRevisions: Record<string, number> = {}
  const flowSessions = { search: createClientSession(), fetch: createClientSession() }
  let pending = $state<Record<string, boolean>>({})
  let errors = $state<Record<string, string>>({})
  let checks = $state<Record<string, number | undefined>>({})
  let preferences = $state<SearchPreferences>({
    maxResults: 5,
    compression: 'none',
    cutoffLimit: 2000,
    blacklist: [],
    modelToolsPreferred: false,
  })
  let blacklist = $state('')
  let loading = $state(true)
  let loadError = $state('')
  let query = $state('')
  let pageUrl = $state('')
  let result = $state<SearchResult | null>(null)
  let page = $state<SearchPage | null>(null)
  const controller = new AbortController()
  const options = { signal: controller.signal }
  let keyDialog: HTMLDialogElement
  let keyTarget = $state<{ cap: SearchCap; provider: string } | null>(null)
  let editingId = $state<string | undefined>()
  let editValue = $state('')
  let password = $state('')
  let revealed = $state<Record<string, string>>({})
  let dialogError = $state('')
  const dialogSession = createClientSession()
  let editRevision = 0
  let passwordRevision = 0

  const keyOf = (cap: SearchCap, provider: string) => `${cap}:${provider}`
  const errorText = (error: unknown) => (isKitClientError(error) ? t(`errors.${error.code}`) : t('errors.internal'))
  const activeProvider = (cap: SearchCap) =>
    settings[cap]?.providers.find((p) => p.id === settings[cap]?.defaultProvider)
  const dialogProvider = $derived.by(() => {
    const target = keyTarget
    return target ? settings[target.cap]?.providers.find((p) => p.id === target.provider) : null
  })
  const dialogBusy = $derived(keyTarget !== null && pending[keyTarget.cap] === true)
  const draftOf = (cap: SearchCap, provider: string) => drafts[keyOf(cap, provider)] ?? {}
  function apply(next: SearchSettingsDto) {
    if (!controller.signal.aborted) settings = { ...settings, [next.capability]: next }
  }
  function setPending(key: string, value: boolean) {
    pending = { ...pending, [key]: value }
  }
  function setError(key: string, value: string) {
    errors = { ...errors, [key]: value }
  }
  function safeUrl(raw?: string) {
    if (!raw) return null
    try {
      const url = new URL(raw)
      return ['https:', 'http:'].includes(url.protocol) ? url.href : null
    } catch {
      return null
    }
  }
  function patchDraft(cap: SearchCap, provider: string, patch: Draft) {
    const key = keyOf(cap, provider)
    drafts = { ...drafts, [key]: { ...drafts[key], ...patch } }
    const revisions = draftRevisions[key] ?? {}
    for (const field of Object.keys(patch) as (keyof Draft)[]) revisions[field] = (revisions[field] ?? 0) + 1
    draftRevisions[key] = revisions
    checks = { ...checks, [key]: undefined }
  }
  async function load() {
    if (!port) return
    loading = true
    loadError = ''
    try {
      const list = await Promise.all(caps.map((cap) => port.settings(cap, options)))
      if (controller.signal.aborted) return
      for (const next of list) apply(next)
      const first = list[0]
      if (!first) throw new Error('Search settings missing')
      preferences = first.preferences
      blacklist = preferences.blacklist.join('\n')
    } catch (error) {
      if (!controller.signal.aborted) loadError = errorText(error)
    } finally {
      if (!controller.signal.aborted) loading = false
    }
  }
  async function choose(cap: SearchCap, provider: string) {
    if (!port || pending[cap]) return
    setPending(cap, true)
    setError(cap, '')
    try {
      apply(await port.setDefault(cap, provider, options))
    } catch (error) {
      if (!controller.signal.aborted) setError(cap, errorText(error))
    } finally {
      setPending(cap, false)
    }
  }
  async function saveAndCheck(cap: SearchCap, provider: SearchProvider, check = false) {
    if (!port || pending[cap]) return
    const key = keyOf(cap, provider.id)
    const draft = { ...draftOf(cap, provider.id) }
    const revisions = { ...draftRevisions[key] }
    const keyRevision = keyRevisions[key] ?? 0
    const value = keyDrafts[key]?.trim()
    const session = flowSessions[cap]
    session.invalidate()
    const token = session.capture()
    const isCurrent = () => !controller.signal.aborted && session.isCurrent(token)
    setPending(cap, true)
    setError(cap, '')
    checks = { ...checks, [key]: undefined }
    try {
      if (value && value.length > 4096) throw new Error('credential too long')
      const saved = await saveSearchSettings(
        port,
        {
          config: {
            capability: cap,
            provider: provider.id,
            ...(draft.baseUrl !== undefined ? { baseUrl: draft.baseUrl.trim() } : {}),
            ...(provider.basicAuth && draft.basicAuthUsername !== undefined
              ? { basicAuthUsername: draft.basicAuthUsername.trim() }
              : {}),
          },
          ...(value ? { key: { value } } : {}),
          check,
        },
        {
          isCurrent,
          onSettings: apply,
          onConfigSaved(sent) {
            const next = { ...drafts[key] }
            for (const field of ['baseUrl', 'basicAuthUsername'] as const) {
              if (sent[field] !== undefined && draftRevisions[key]?.[field] === revisions[field]) delete next[field]
            }
            drafts = { ...drafts, [key]: next }
          },
          onKeySaved() {
            if ((keyRevisions[key] ?? 0) === keyRevision) keyDrafts = { ...keyDrafts, [key]: '' }
          },
        },
        options,
      )
      if (!isCurrent()) return
      if (saved.checked) checks = { ...checks, [key]: saved.checked.count }
      else notify.toast('success', t('search.saved'))
    } catch (error) {
      if (isCurrent()) setError(cap, errorText(error))
    } finally {
      if (isCurrent()) setPending(cap, false)
    }
  }
  function clearDialog() {
    dialogSession.invalidate()
    if (keyTarget) keyDrafts = { ...keyDrafts, [keyOf(keyTarget.cap, keyTarget.provider)]: '' }
    keyTarget = null
    editingId = undefined
    editRevision++
    editValue = ''
    passwordRevision++
    password = ''
    revealed = {}
    dialogError = ''
  }
  function openKeys(cap: SearchCap, provider: string) {
    clearDialog()
    keyTarget = { cap, provider }
    keyDialog.showModal()
  }
  function invalidateCredentials() {
    dialogSession.invalidate()
    passwordRevision++
    password = ''
    revealed = {}
    dialogError = ''
  }
  async function saveEditedKey() {
    if (!port || !keyTarget || dialogBusy || !editValue.trim()) return
    const target = { ...keyTarget }
    const value = editValue.trim()
    const id = editingId
    const revision = editRevision
    invalidateCredentials()
    const token = dialogSession.capture()
    const isCurrent = () => !controller.signal.aborted && dialogSession.isCurrent(token)
    setPending(target.cap, true)
    try {
      const next = await port.saveKey(target.cap, target.provider, value, id, options)
      if (!isCurrent()) return
      apply(next)
      if (revision === editRevision) {
        editValue = ''
        editingId = undefined
        editRevision++
      }
    } catch (error) {
      if (isCurrent()) dialogError = errorText(error)
    } finally {
      if (!controller.signal.aborted) setPending(target.cap, false)
    }
  }
  async function removeKey(key: SearchKey) {
    if (!port || !keyTarget || dialogBusy) return
    const target = { ...keyTarget }
    const revision = editRevision
    invalidateCredentials()
    const token = dialogSession.capture()
    const isCurrent = () => !controller.signal.aborted && dialogSession.isCurrent(token)
    setPending(target.cap, true)
    try {
      const next = await port.removeKey(target.cap, target.provider, key.id, options)
      if (!isCurrent()) return
      apply(next)
      if (editingId === key.id && revision === editRevision) {
        editingId = undefined
        editValue = ''
        editRevision++
      }
    } catch (error) {
      if (isCurrent()) dialogError = errorText(error)
    } finally {
      if (!controller.signal.aborted) setPending(target.cap, false)
    }
  }
  async function revealKey(key: SearchKey, copy = false) {
    if (!port || !keyTarget || dialogBusy) return
    const target = { ...keyTarget }
    const token = dialogSession.capture()
    const revision = passwordRevision
    const currentPassword = port.revealNeedsPassword ? password : undefined
    const isCurrent = () => !controller.signal.aborted && dialogSession.isCurrent(token)
    setPending(target.cap, true)
    dialogError = ''
    try {
      const value = await port.reveal(target.cap, target.provider, key.id, currentPassword, options)
      if (!isCurrent()) return
      if (copy) {
        await navigator.clipboard.writeText(value)
        if (isCurrent()) notify.toast('success', t('search.copied'))
      } else revealed = { ...revealed, [key.id]: value }
    } catch (error) {
      if (isCurrent()) dialogError = copy && !isKitClientError(error) ? t('search.copyFailed') : errorText(error)
    } finally {
      if (isCurrent() && passwordRevision === revision) {
        password = ''
        passwordRevision++
      }
      if (!controller.signal.aborted) setPending(target.cap, false)
    }
  }
  async function savePreferences() {
    if (!port || pending.preferences) return
    setPending('preferences', true)
    setError('preferences', '')
    try {
      const saved = await port.savePreferences(
        {
          ...preferences,
          blacklist: blacklist
            .split('\n')
            .map((rule) => rule.trim())
            .filter(Boolean),
        },
        options,
      )
      if (!controller.signal.aborted) {
        preferences = saved
        blacklist = saved.blacklist.join('\n')
        notify.toast('success', t('search.saved'))
      }
    } catch (error) {
      if (!controller.signal.aborted) setError('preferences', errorText(error))
    } finally {
      setPending('preferences', false)
    }
  }
  async function search() {
    if (!port || pending.query || !query.trim()) return
    setPending('query', true)
    setError('query', '')
    result = null
    try {
      const next = await port.search(query.trim(), options)
      if (!controller.signal.aborted) result = next
    } catch (error) {
      if (!controller.signal.aborted) setError('query', errorText(error))
    } finally {
      setPending('query', false)
    }
  }
  async function readPage() {
    if (!port || pending.page || !pageUrl.trim()) return
    setPending('page', true)
    setError('page', '')
    page = null
    try {
      const next = await port.fetch(pageUrl.trim(), options)
      if (!controller.signal.aborted) page = next
    } catch (error) {
      if (!controller.signal.aborted) setError('page', errorText(error))
    } finally {
      setPending('page', false)
    }
  }
  onMount(() => {
    void load()
    return () => {
      controller.abort()
      for (const session of Object.values(flowSessions)) session.invalidate()
      keyDrafts = {}
      keyDialog?.close()
      clearDialog()
    }
  })
</script>

{#if !port}
  <Empty.Root><Empty.Description>{t('search.unavailable')}</Empty.Description></Empty.Root>
{:else if loading}
  <p class="text-muted-foreground text-kit-sm">{t('search.loading')}</p>
{:else if loadError}
  <Alert.Root variant="destructive"><Alert.Description>{loadError}</Alert.Description></Alert.Root>
  <Button variant="outline" onclick={() => void load()}>{t('chat.retry')}</Button>
{:else}
  <div class="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-4">
    {#each caps as cap (cap)}
      {@const provider = activeProvider(cap)}
      {#if provider}
        {@const key = keyOf(cap, provider.id)}
        <section class="flex min-w-0 flex-col gap-3 rounded-kit border p-4" aria-labelledby={`search-${cap}`}>
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
              disabled={pending[cap]}
              value={settings[cap]?.defaultProvider}
              onchange={(event) => void choose(cap, event.currentTarget.value)}
            >
              {#each settings[cap]?.providers ?? [] as option (option.id)}<option value={option.id}
                  >{option.name}</option
                >{/each}
            </select>
          </header>
          {#if errors[cap]}<Alert.Root variant="destructive"
              ><Alert.Description>{errors[cap]}</Alert.Description></Alert.Root
            >{/if}
          <Field.Group class="gap-3 border-t pt-3">
            {#if provider.keyMode !== 'none' || provider.basicAuth}
              <Field.Field data-disabled={pending[cap]}>
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
                    disabled={pending[cap]}
                    placeholder={t('search.newKey')}
                    value={keyDrafts[key] ?? ''}
                    oninput={(event) => {
                      keyDrafts = { ...keyDrafts, [key]: event.currentTarget.value }
                      keyRevisions[key] = (keyRevisions[key] ?? 0) + 1
                      checks = { ...checks, [key]: undefined }
                    }}
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="icon-sm"
                    disabled={pending[cap]}
                    aria-label={t('search.manageKeys')}
                    title={t('search.manageKeys')}
                    onclick={() => openKeys(cap, provider.id)}><ListIcon /></Button
                  >
                  <Button
                    type="button"
                    variant="outline"
                    size="icon-sm"
                    disabled={pending[cap]}
                    aria-label={t('search.check')}
                    title={t('search.check')}
                    onclick={() => void saveAndCheck(cap, provider, true)}
                    >{#if pending[cap]}<LoaderCircleIcon class="animate-spin" />{:else}<ActivityIcon />{/if}</Button
                  >
                </div>
                <Field.Description
                  >{provider.keys.length
                    ? provider.keys.map((item) => item.masked).join(' · ')
                    : t('search.keyNotSet')}</Field.Description
                >
              </Field.Field>
            {/if}
            {#if provider.defaultBaseUrl !== undefined}
              <Field.Field data-disabled={pending[cap]}>
                <Field.Label for={`search-url-${cap}`}>{t('search.baseUrl')}</Field.Label>
                <div class="flex min-w-0 items-center gap-2">
                  <CopyInput
                    id={`search-url-${cap}`}
                    class="min-w-0 flex-1"
                    disabled={pending[cap]}
                    value={draftOf(cap, provider.id).baseUrl ?? provider.baseUrl ?? provider.defaultBaseUrl}
                    oninput={(event) => patchDraft(cap, provider.id, { baseUrl: event.currentTarget.value })}
                  />
                  {#if provider.keyMode === 'none' && !provider.basicAuth}<Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      disabled={pending[cap]}
                      aria-label={t('search.check')}
                      title={t('search.check')}
                      onclick={() => void saveAndCheck(cap, provider, true)}><ActivityIcon /></Button
                    >{/if}
                </div>
              </Field.Field>
            {/if}
            {#if provider.basicAuth}
              <Field.Field
                ><Field.Label for={`search-user-${cap}`}>{t('search.username')}</Field.Label><Input
                  id={`search-user-${cap}`}
                  disabled={pending[cap]}
                  value={draftOf(cap, provider.id).basicAuthUsername ?? provider.basicAuthUsername ?? ''}
                  oninput={(event) => patchDraft(cap, provider.id, { basicAuthUsername: event.currentTarget.value })}
                /></Field.Field
              >
            {/if}
          </Field.Group>
          <div class="flex flex-wrap items-center gap-2">
            {#if provider.defaultBaseUrl !== undefined || provider.keyMode !== 'none'}<Button
                type="button"
                size="sm"
                disabled={pending[cap]}
                onclick={() => void saveAndCheck(cap, provider)}>{t('search.save')}</Button
              >{/if}
            {#if pending[cap]}<span class="text-muted-foreground text-kit-sm" role="status">{t('search.checking')}</span
              >{:else if checks[key] !== undefined}<span class="text-muted-foreground text-kit-sm" role="status"
                >{t('search.resultCount', { count: checks[key] ?? 0 })}</span
              >{/if}
          </div>
        </section>
      {/if}
    {/each}
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
              disabled={pending.preferences}
              bind:value={preferences.maxResults}
            /></Field.Field
          >
          <Field.Field orientation="horizontal"
            ><Field.Label for="search-compression">{t('search.compression')}</Field.Label><select
              id="search-compression"
              class="bg-background border-input h-kit-control ml-auto rounded-kit-sm border px-2 text-kit-sm"
              disabled={pending.preferences}
              bind:value={preferences.compression}
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
                disabled={pending.preferences}
                bind:value={preferences.cutoffLimit}
              /></Field.Field
            >{/if}
          <Field.Field
            ><Field.Label for="search-blacklist">{t('search.blacklist')}</Field.Label><Textarea
              id="search-blacklist"
              rows={3}
              disabled={pending.preferences}
              placeholder="example.com\n*://*.example.org/*"
              bind:value={blacklist}
            /></Field.Field
          >
        </Field.Group>
      </details>
      <Field.Field class="mt-3 border-t pt-3"
        ><label class="flex items-center gap-2 text-kit-sm"
          ><input type="checkbox" disabled={pending.preferences} bind:checked={preferences.modelToolsPreferred} />{t(
            'search.nativePreferred',
          )}</label
        ><Field.Description>{t('search.nativePrivacy')}</Field.Description></Field.Field
      >
      {#if errors.preferences}<p role="alert" class="text-destructive text-kit-sm">{errors.preferences}</p>{/if}
      <Button class="mt-3" type="button" size="sm" disabled={pending.preferences} onclick={() => void savePreferences()}
        >{t('search.save')}</Button
      >
    </section>
    <section class="flex min-w-0 flex-col gap-3 rounded-kit border p-4" aria-label={t('search.tools')}>
      <Field.Field
        ><Field.Label for="search-query">{t('search.query')}</Field.Label>
        <div class="flex min-w-0 gap-2">
          <Input id="search-query" class="min-w-0 flex-1" disabled={pending.query} bind:value={query} /><Button
            type="button"
            disabled={pending.query || !query.trim()}
            onclick={() => void search()}>{t('search.runSearch')}</Button
          >
        </div></Field.Field
      >
      {#if errors.query}<p class="text-destructive text-kit-sm" role="alert">{errors.query}</p>{/if}
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
          <Input id="search-page-url" class="min-w-0 flex-1" disabled={pending.page} bind:value={pageUrl} /><Button
            type="button"
            disabled={pending.page || !pageUrl.trim()}
            onclick={() => void readPage()}>{t('search.read')}</Button
          >
        </div></Field.Field
      >
      {#if errors.page}<p class="text-destructive text-kit-sm" role="alert">{errors.page}</p>{/if}
      {#if page}<h3 class="text-kit-base font-medium">{page.title}</h3>
        <a
          class="text-primary break-words text-kit-sm"
          href={safeUrl(page.url) ?? ''}
          target="_blank"
          rel="noopener noreferrer">{page.url}</a
        >
        <pre class="max-h-96 overflow-auto whitespace-pre-wrap break-words text-kit-sm">{page.content}</pre>{/if}
    </section>
  </div>
{/if}

<dialog
  bind:this={keyDialog}
  aria-labelledby="search-key-dialog-title"
  onclose={clearDialog}
  class="bg-background text-foreground border-border m-auto max-h-[80dvh] w-[min(560px,calc(100vw-32px))] overflow-y-auto rounded-kit border p-4"
>
  <h2 id="search-key-dialog-title" class="text-kit-base font-semibold">
    {t('search.manageKeys')}{#if dialogProvider}
      · {dialogProvider.name}{/if}
  </h2>
  {#if dialogProvider}
    <Field.Group class="mt-3 gap-3">
      {#if port?.revealNeedsPassword}<Field.Field
          ><Field.Label for="search-key-password">{t('search.currentPassword')}</Field.Label><Input
            id="search-key-password"
            type="password"
            autocomplete="off"
            disabled={dialogBusy}
            value={password}
            oninput={(event) => {
              password = event.currentTarget.value
              passwordRevision++
            }}
          /></Field.Field
        >{/if}
      <ul class="flex flex-col gap-3">
        {#each dialogProvider.keys as key (key.id)}
          <li class="flex min-w-0 flex-col gap-1 border-b pb-2 text-kit-sm">
            <span class="break-all font-mono">{revealed[key.id] ?? key.masked}</span>
            <div class="flex flex-wrap gap-1">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={dialogBusy}
                onclick={() => {
                  invalidateCredentials()
                  editingId = key.id
                  editValue = ''
                  editRevision++
                }}>{t('search.editKey')}</Button
              >
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={dialogBusy || (port?.revealNeedsPassword && !password)}
                onclick={() => void revealKey(key)}>{t('search.reveal')}</Button
              >
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={dialogBusy || (port?.revealNeedsPassword && !password)}
                onclick={() => void revealKey(key, true)}>{t('search.copy')}</Button
              >
              <Button type="button" variant="ghost" size="sm" disabled={dialogBusy} onclick={() => void removeKey(key)}
                >{t('search.delete')}</Button
              >
            </div>
          </li>
        {/each}
      </ul>
      <Field.Field
        ><Field.Label for="search-edit-key">{t(editingId ? 'search.editKey' : 'search.newKey')}</Field.Label><Input
          id="search-edit-key"
          type="password"
          autocomplete="off"
          spellcheck={false}
          maxlength={4096}
          disabled={dialogBusy}
          value={editValue}
          oninput={(event) => {
            editValue = event.currentTarget.value
            editRevision++
          }}
        /></Field.Field
      >
      {#if dialogError}<p class="text-destructive text-kit-sm" role="alert">{dialogError}</p>{/if}
      <div class="flex flex-wrap justify-end gap-2">
        <Button
          type="button"
          size="sm"
          disabled={dialogBusy || !editValue.trim() || (!editingId && dialogProvider.keys.length >= 16)}
          onclick={() => void saveEditedKey()}>{t(editingId ? 'search.save' : 'search.addKey')}</Button
        ><Button type="button" variant="outline" size="sm" onclick={() => keyDialog.close()}
          >{t('common.cancel')}</Button
        >
      </div>
    </Field.Group>
  {/if}
</dialog>
