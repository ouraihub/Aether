<script lang="ts">
  import { onMount } from 'svelte'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import SearchProviderCard from './SearchProviderCard.svelte'
  import SearchAdvancedTools from './SearchAdvancedTools.svelte'
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
  const errorText = (error: unknown) =>
    isKitClientError(error) ? t(`errors.${error.code}`) : t('errors.internal')
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
  function patchDraft(cap: SearchCap, provider: string, patch: Draft) {
    const key = keyOf(cap, provider)
    drafts = { ...drafts, [key]: { ...drafts[key], ...patch } }
    const revisions = draftRevisions[key] ?? {}
    for (const field of Object.keys(patch) as (keyof Draft)[])
      revisions[field] = (revisions[field] ?? 0) + 1
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
              if (sent[field] !== undefined && draftRevisions[key]?.[field] === revisions[field])
                delete next[field]
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
      if (isCurrent())
        dialogError = copy && !isKitClientError(error) ? t('search.copyFailed') : errorText(error)
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
      {@const current = settings[cap]}
      {#if provider && current}
        {@const key = keyOf(cap, provider.id)}
        <SearchProviderCard
          {cap}
          {provider}
          settings={current}
          pending={pending[cap] === true}
          error={errors[cap] ?? ''}
          draft={draftOf(cap, provider.id)}
          keyDraft={keyDrafts[key] ?? ''}
          check={checks[key]}
          onChoose={(next) => void choose(cap, next)}
          onOpenKeys={() => openKeys(cap, provider.id)}
          onKeyInput={(value) => {
            keyDrafts = { ...keyDrafts, [key]: value }
            keyRevisions[key] = (keyRevisions[key] ?? 0) + 1
            checks = { ...checks, [key]: undefined }
          }}
          onPatchDraft={(patch) => patchDraft(cap, provider.id, patch)}
          onSave={(check = false) => void saveAndCheck(cap, provider, check)}
        />
      {/if}
    {/each}
    <SearchAdvancedTools
      bind:preferences
      bind:blacklist
      bind:query
      bind:pageUrl
      preferencesPending={pending.preferences === true}
      preferencesError={errors.preferences ?? ''}
      {result}
      {page}
      queryPending={pending.query === true}
      pagePending={pending.page === true}
      queryError={errors.query ?? ''}
      pageError={errors.page ?? ''}
      onSavePreferences={() => void savePreferences()}
      onSearch={() => void search()}
      onReadPage={() => void readPage()}
    />
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
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={dialogBusy}
                onclick={() => void removeKey(key)}>{t('search.delete')}</Button
              >
            </div>
          </li>
        {/each}
      </ul>
      <Field.Field
        ><Field.Label for="search-edit-key"
          >{t(editingId ? 'search.editKey' : 'search.newKey')}</Field.Label
        ><Input
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
          disabled={dialogBusy ||
            !editValue.trim() ||
            (!editingId && dialogProvider.keys.length >= 16)}
          onclick={() => void saveEditedKey()}
          >{t(editingId ? 'search.save' : 'search.addKey')}</Button
        ><Button type="button" variant="outline" size="sm" onclick={() => keyDialog.close()}
          >{t('common.cancel')}</Button
        >
      </div>
    </Field.Group>
  {/if}
</dialog>
