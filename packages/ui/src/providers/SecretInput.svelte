<script lang="ts">
  // API key row (Cherry ApiKey): masked value with show/copy/edit/delete; a new key commits on blur or Enter.
  import CopyIcon from '@lucide/svelte/icons/copy'
  import EditIcon from '@lucide/svelte/icons/pencil'
  import EyeIcon from '@lucide/svelte/icons/eye'
  import EyeOffIcon from '@lucide/svelte/icons/eye-off'
  import TrashIcon from '@lucide/svelte/icons/trash-2'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import type { Endpoint } from '../ports'

  let {
    id,
    saved,
    pending,
    oncommit,
  }: {
    id: string
    saved: Endpoint | null
    /** Unsaved key text (`''` = clear pending); null when nothing is pending. */
    pending: string | null
    /** `''` clears the stored key. */
    oncommit: (key: string) => void
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  let editing = $state(false),
    askingPassword = $state(false),
    password = $state(''),
    revealed = $state<string | null>(null),
    copyAfterReveal = false,
    text = $state(''),
    busy = $state(false)
  const hasKey = $derived(pending ? true : pending === '' ? false : (saved?.hasKey ?? false))
  const shown = $derived(
    revealed ??
      (pending
        ? '••••••••'
        : saved?.keyReadable
          ? (saved.keyMasked?.replace(/\*{4,}/, '****') ?? t('providers.key_none'))
          : t('providers.key_unreadable')),
  )

  async function reveal() {
    if (!ports.providers || !saved?.hasKey || pending !== null || busy) return
    if (ports.providers.revealNeedsPassword && !password) {
      askingPassword = true
      return
    }
    busy = true
    try {
      revealed = await ports.providers.revealKey(saved.id, password)
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
    if (revealed === null) {
      copyAfterReveal = true
      return reveal()
    }
    try {
      await navigator.clipboard.writeText(revealed)
      notify.toast('success', t('providers.key_copied'))
    } catch {
      notify.toast('error', t('providers.key_copy_failed'))
    }
  }
  function commit() {
    const key = text.trim()
    if (key) {
      revealed = null
      oncommit(key)
    }
    text = ''
    editing = false
  }
  async function clear() {
    if (!(await notify.confirm({ title: t('providers.key_delete'), body: t('providers.key_delete_confirm') }))) return
    revealed = null
    oncommit('')
  }
</script>

<div class="flex min-w-0 flex-1 flex-col">
  {#if editing || !hasKey}
    <Input
      {id}
      type="password"
      autocomplete="off"
      spellcheck={false}
      class="min-w-0 flex-1 font-mono"
      placeholder={t('providers.key_placeholder')}
      autofocus={editing}
      bind:value={text}
      onblur={commit}
      onkeydown={(event) => {
        if (event.key === 'Enter') event.currentTarget.blur()
        if (event.key === 'Escape' && editing) {
          text = ''
          editing = false
        }
      }}
    />
  {:else}
    <div
      {id}
      class="group/key flex h-kit-control min-w-0 flex-1 items-center gap-1 rounded-kit border bg-muted/30 pr-1.5 pl-2.5 font-mono text-kit-sm transition-colors hover:border-ring/60 focus-within:border-ring"
    >
      <span class="min-w-0 flex-1 truncate">{shown}</span>
      <div
        class="pointer-events-none flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover/key:pointer-events-auto group-hover/key:opacity-100 group-focus-within/key:pointer-events-auto group-focus-within/key:opacity-100"
      >
        {#if saved?.hasKey && pending === null}<Button
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
          onclick={() => {
            revealed = null
            editing = true
          }}><EditIcon /></Button
        >
        <Button
          variant="ghost"
          size="icon-xs"
          class="hover:bg-destructive/10 hover:text-destructive"
          title={t('providers.key_delete')}
          aria-label={t('providers.key_delete')}
          onclick={() => void clear()}><TrashIcon /></Button
        >
      </div>
      {#if saved?.hasKey && pending === null}<Button
          variant="ghost"
          size="icon-xs"
          class="text-muted-foreground"
          title={revealed === null ? t('providers.key_show') : t('providers.key_hide')}
          aria-label={revealed === null ? t('providers.key_show') : t('providers.key_hide')}
          disabled={busy}
          onclick={() => {
            copyAfterReveal = false
            if (revealed === null) void reveal()
            else revealed = null
          }}
          >{#if revealed === null}<EyeIcon />{:else}<EyeOffIcon />{/if}</Button
        >{/if}
    </div>
  {/if}
  {#if askingPassword}
    <Input
      type="password"
      class="mt-2"
      placeholder={t('providers.key_current_password')}
      aria-label={t('providers.key_current_password')}
      bind:value={password}
      onkeydown={(event) => event.key === 'Enter' && void reveal()}
    />
  {/if}
</div>
