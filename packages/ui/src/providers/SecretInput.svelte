<script lang="ts">
  // API key row: primary key masked with `+N`, show/copy/edit/delete; 🔑 opens key management.
  import { onDestroy, untrack } from 'svelte'
  import { createClientSession } from '../session'
  import CopyIcon from '@lucide/svelte/icons/copy'
  import EditIcon from '@lucide/svelte/icons/pencil'
  import EyeIcon from '@lucide/svelte/icons/eye'
  import EyeOffIcon from '@lucide/svelte/icons/eye-off'
  import KeyRoundIcon from '@lucide/svelte/icons/key-round'
  import TrashIcon from '@lucide/svelte/icons/trash-2'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import type { Endpoint, Provider, ProvidersPort } from '../ports'
  import { keyError, maskedText, primaryKey } from './draft'
  import EndpointKeysDialog from './EndpointKeysDialog.svelte'

  let {
    id,
    endpoint,
    onchanged,
  }: {
    id: string
    /** `null` while a new endpoint is still being stored. */
    endpoint: Endpoint | null
    onchanged: (providers: Provider[]) => void
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  let askingPassword = $state(false),
    password = $state(''),
    revealed = $state<{ id: string; text: string } | null>(null),
    copyAfterReveal = false,
    text = $state(''),
    busy = $state(false),
    dialogOpen = $state(false),
    editKeyId = $state<string | null>(null)
  const keys = $derived(endpoint?.keys ?? [])
  const primary = $derived(primaryKey(keys))
  const plain = $derived(revealed && revealed.id === primary?.id ? revealed.text : null)
  const session = createClientSession()
  let passwordRevision = 0
  let textRevision = 0
  function clear() {
    session.invalidate()
    password = ''
    passwordRevision++
    text = ''
    textRevision++
    revealed = null
    askingPassword = false
    copyAfterReveal = false
    busy = false
  }
  $effect.pre(() => {
    const target = `${endpoint?.id ?? ''}:${JSON.stringify(keys)}`
    const managing = dialogOpen
    untrack(() => {
      void target
      void managing
      clear()
    })
  })
  onDestroy(clear)

  async function reveal() {
    const port = ports.providers
    const key = primary
    if (!port || !key?.readable || busy) return
    if (port.revealNeedsPassword && !password) {
      askingPassword = true
      return
    }
    const token = session.capture()
    const revision = passwordRevision
    const copying = copyAfterReveal
    busy = true
    try {
      const value = await port.revealKey(key.id, password)
      if (!session.isCurrent(token)) return
      revealed = { id: key.id, text: value }
      askingPassword = false
      if (copying) await copy()
    } catch (error) {
      if (session.isCurrent(token))
        notify.toast('error', error instanceof Error ? error.message : t('providers.key_reveal_failed'))
    } finally {
      if (session.isCurrent(token)) {
        busy = false
        if (passwordRevision === revision) password = ''
        copyAfterReveal = false
      }
    }
  }
  /** Check the generation again immediately before the irreversible clipboard write. */
  async function copy() {
    if (plain === null) {
      copyAfterReveal = true
      return reveal()
    }
    const token = session.capture()
    const value = plain
    try {
      if (!session.isCurrent(token)) return
      await navigator.clipboard.writeText(value)
      if (session.isCurrent(token)) notify.toast('success', t('providers.key_copied'))
    } catch {
      if (session.isCurrent(token)) notify.toast('error', t('providers.key_copy_failed'))
    }
  }
  async function run(action: (port: ProvidersPort) => Promise<Provider[]>) {
    if (!ports.providers || busy) return false
    const token = session.capture()
    busy = true
    try {
      const next = await action(ports.providers)
      if (!session.isCurrent(token)) return false
      onchanged(next)
      return true
    } catch (error) {
      if (session.isCurrent(token)) notify.toast('error', keyError(error, t))
      return false
    } finally {
      if (session.isCurrent(token)) busy = false
    }
  }
  async function commit() {
    const key = text.trim()
    const endpointId = endpoint?.id
    if (!key || !endpointId || busy) return
    const revision = textRevision
    session.invalidate()
    revealed = null
    password = ''
    askingPassword = false
    if (await run((port) => port.saveKey({ endpointId, key }))) {
      if (textRevision === revision) text = ''
    }
  }
  async function remove() {
    const endpointId = endpoint?.id
    const keyId = primary?.id
    if (!endpointId || !keyId) return
    const token = session.capture()
    if (
      !(await notify.confirm({ title: t('providers.key_delete'), body: t('providers.key_delete_confirm') })) ||
      !session.isCurrent(token)
    )
      return
    clear()
    await run((port) => port.removeKey(endpointId, keyId))
  }
  function manage(keyId: string | null) {
    clear()
    editKeyId = keyId
    dialogOpen = true
  }
</script>

<div class="flex min-w-0 flex-1 flex-col">
  <div class="flex min-w-0 items-start gap-2">
    {#if !primary}
      <!-- An API key is not a login password: type="password" lets browsers autofill (and auto-save) the login one. -->
      <Input
        {id}
        type="text"
        autocomplete="off"
        name="provider-api-key"
        spellcheck={false}
        class="min-w-0 flex-1 font-mono [-webkit-text-security:disc]"
        placeholder={t('providers.key_placeholder')}
        disabled={!endpoint || busy}
        bind:value={text}
        oninput={() => textRevision++}
        onblur={commit}
        onkeydown={(event) => {
          if (event.key === 'Enter') event.currentTarget.blur()
          if (event.key === 'Escape') text = ''
        }}
      />
    {:else}
      <div
        class="group/key flex h-kit-control min-w-0 flex-1 items-center gap-1 rounded-kit border bg-muted/30 pr-1.5 font-mono text-kit-sm transition-colors hover:border-ring/60 focus-within:border-ring"
      >
        <button
          {id}
          type="button"
          aria-haspopup="dialog"
          class="flex h-full min-w-0 flex-1 items-center gap-1.5 pl-2.5 text-left outline-none"
          onclick={() => manage(null)}
        >
          {#if primary.label}<span class="shrink-0 font-sans text-muted-foreground">{primary.label}</span>{/if}
          <span class="min-w-0 truncate">{plain ?? maskedText(primary, t)}</span>
          {#if keys.length > 1}<span class="shrink-0 font-sans text-kit-xs text-muted-foreground"
              >+{keys.length - 1}</span
            >{/if}
        </button>
        <div
          class="pointer-events-none flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover/key:pointer-events-auto group-hover/key:opacity-100 group-focus-within/key:pointer-events-auto group-focus-within/key:opacity-100"
        >
          {#if primary.readable}<Button
              variant="ghost"
              size="icon-xs"
              title={t('providers.key_copy')}
              aria-label={t('providers.key_copy')}
              disabled={busy}
              onclick={() => void copy()}><CopyIcon /></Button
            >{/if}
          <Button
            variant="ghost"
            size="icon-xs"
            title={t('providers.key_edit')}
            aria-label={t('providers.key_edit')}
            onclick={() => manage(primary?.id ?? null)}><EditIcon /></Button
          >
          {#if keys.length === 1}<Button
              variant="ghost"
              size="icon-xs"
              class="hover:bg-destructive/10 hover:text-destructive"
              title={t('providers.key_delete')}
              aria-label={t('providers.key_delete')}
              disabled={busy}
              onclick={() => void remove()}><TrashIcon /></Button
            >{/if}
        </div>
        {#if primary.readable}<Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            title={plain === null ? t('providers.key_show') : t('providers.key_hide')}
            aria-label={plain === null ? t('providers.key_show') : t('providers.key_hide')}
            disabled={busy}
            onclick={() => {
              copyAfterReveal = false
              if (plain === null) void reveal()
              else clear()
            }}
            >{#if plain === null}<EyeIcon />{:else}<EyeOffIcon />{/if}</Button
          >{/if}
      </div>
    {/if}
    <Button
      variant="outline"
      size="icon"
      class="shrink-0"
      title={t('providers.keys_manage')}
      aria-label={t('providers.keys_manage')}
      disabled={!endpoint}
      onclick={() => manage(null)}><KeyRoundIcon /></Button
    >
  </div>
  {#if askingPassword}
    <Input
      type="password"
      autocomplete="new-password"
      class="mt-2"
      placeholder={t('providers.key_current_password')}
      aria-label={t('providers.key_current_password')}
      bind:value={password}
      oninput={() => passwordRevision++}
      onkeydown={(event) => event.key === 'Enter' && void reveal()}
    />
  {/if}
</div>

{#if endpoint}
  <EndpointKeysDialog
    bind:open={dialogOpen}
    {endpoint}
    {editKeyId}
    onchanged={(next) => {
      clear()
      onchanged(next)
    }}
  />
{/if}
