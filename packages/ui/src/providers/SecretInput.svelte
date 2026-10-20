<script lang="ts">
  // API key row (Cherry ApiKey): primary key masked with `+N`, show/copy/edit/delete; 🔑 opens key management.
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

  async function reveal() {
    const port = ports.providers
    const key = primary
    if (!port || !key?.readable || busy) return
    if (port.revealNeedsPassword && !password) {
      askingPassword = true
      return
    }
    busy = true
    try {
      revealed = { id: key.id, text: await port.revealKey(key.id, password) }
      askingPassword = false
      if (copyAfterReveal) await copy()
    } catch (error) {
      notify.toast('error', error instanceof Error ? error.message : t('providers.key_reveal_failed'))
    } finally {
      busy = false
      password = ''
      copyAfterReveal = false
    }
  }
  /** Copy needs the plaintext; without it the reveal (and its password prompt on the web) runs first. */
  async function copy() {
    if (plain === null) {
      copyAfterReveal = true
      return reveal()
    }
    try {
      await navigator.clipboard.writeText(plain)
      notify.toast('success', t('providers.key_copied'))
    } catch {
      notify.toast('error', t('providers.key_copy_failed'))
    }
  }
  async function run(action: (port: ProvidersPort) => Promise<Provider[]>) {
    if (!ports.providers || busy) return
    busy = true
    try {
      onchanged(await action(ports.providers))
    } catch (error) {
      notify.toast('error', keyError(error, t))
    } finally {
      busy = false
    }
  }
  function commit() {
    const key = text.trim()
    text = ''
    const endpointId = endpoint?.id
    if (key && endpointId) void run((port) => port.saveKey({ endpointId, key }))
  }
  async function remove() {
    const endpointId = endpoint?.id
    const keyId = primary?.id
    if (!endpointId || !keyId) return
    if (!(await notify.confirm({ title: t('providers.key_delete'), body: t('providers.key_delete_confirm') }))) return
    revealed = null
    await run((port) => port.removeKey(endpointId, keyId))
  }
  function manage(keyId: string | null) {
    editKeyId = keyId
    dialogOpen = true
  }
</script>

<div class="flex min-w-0 flex-1 flex-col">
  <div class="flex min-w-0 items-start gap-2">
    {#if !primary}
      <Input
        {id}
        type="password"
        autocomplete="new-password"
        spellcheck={false}
        class="min-w-0 flex-1 font-mono"
        placeholder={t('providers.key_placeholder')}
        disabled={!endpoint || busy}
        bind:value={text}
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
              else revealed = null
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
      autocomplete="current-password"
      class="mt-2"
      placeholder={t('providers.key_current_password')}
      aria-label={t('providers.key_current_password')}
      bind:value={password}
      onkeydown={(event) => event.key === 'Enter' && void reveal()}
    />
  {/if}
</div>

{#if endpoint}
  <EndpointKeysDialog bind:open={dialogOpen} {endpoint} {editKeyId} {onchanged} />
{/if}
