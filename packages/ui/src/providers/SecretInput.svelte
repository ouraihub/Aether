<script lang="ts">
  import CopyIcon from '@lucide/svelte/icons/copy'
  import EditIcon from '@lucide/svelte/icons/pencil'
  import EyeIcon from '@lucide/svelte/icons/eye'
  import EyeOffIcon from '@lucide/svelte/icons/eye-off'
  import TrashIcon from '@lucide/svelte/icons/trash-2'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import type { Endpoint } from '../ports'

  let { id, saved, value = $bindable() }: { id: string; saved: Endpoint | null; value: string | null } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  let replacing = $state(false),
    askingPassword = $state(false),
    password = $state(''),
    revealed = $state<string | null>(null),
    text = $state(''),
    busy = $state(false)
  const shown = $derived(
    revealed ??
      (saved?.keyReadable
        ? (saved.keyMasked ?? t('providers.key_none'))
        : saved
          ? t('providers.key_unreadable')
          : t('providers.key_none')),
  )

  async function reveal() {
    if (!ports.providers || !saved?.hasKey || busy) return
    if (ports.providers.revealNeedsPassword && !password) {
      askingPassword = true
      return
    }
    busy = true
    try {
      revealed = await ports.providers.revealKey(saved.id, password)
    } catch (error) {
      notify.toast('error', error instanceof Error ? error.message : t('providers.key_reveal_failed'))
    } finally {
      busy = false
      password = ''
    }
  }
  async function copy() {
    if (revealed === null) return
    try {
      await navigator.clipboard.writeText(revealed)
      notify.toast('success', t('providers.key_copied'))
    } catch {
      notify.toast('error', t('providers.key_copy_failed'))
    }
  }
  function edit() {
    replacing = true
    askingPassword = false
    revealed = null
    text = ''
  }
  function input(next: string) {
    text = next
    value = next === '' ? '' : next
  }
  function clear() {
    replacing = false
    askingPassword = false
    revealed = null
    text = ''
    value = ''
  }
</script>

{#if value === ''}
  <div class="flex min-w-0 flex-1 items-center rounded-kit border bg-muted/20 px-3 py-2 font-mono text-kit-sm">
    <span class="truncate">{t('providers.key_cleared')}</span>
  </div>
{:else if replacing || saved === null || !saved.hasKey}
  <Input
    {id}
    type="password"
    autocomplete="off"
    class="min-w-0 flex-1 font-mono"
    placeholder={t('providers.key_none')}
    value={text}
    oninput={(event) => input(event.currentTarget.value)}
  />
{:else}
  <div class="flex min-w-0 flex-1 items-center rounded-kit border bg-muted/20 px-3 py-2 font-mono text-kit-sm">
    <span class="truncate">{shown}</span>
    <div class="ml-auto flex shrink-0 items-center gap-0.5">
      {#if revealed !== null}<Button
          variant="ghost"
          size="icon-xs"
          title={t('providers.key_copy')}
          aria-label={t('providers.key_copy')}
          onclick={() => void copy()}><CopyIcon /></Button
        >{/if}
      <Button
        variant="ghost"
        size="icon-xs"
        title={t('providers.key_edit')}
        aria-label={t('providers.key_edit')}
        onclick={edit}><EditIcon /></Button
      >
      <Button
        variant="ghost"
        size="icon-xs"
        title={t('providers.key_delete')}
        aria-label={t('providers.key_delete')}
        onclick={clear}><TrashIcon /></Button
      >
      <Button
        variant="ghost"
        size="icon-xs"
        title={revealed === null ? t('providers.key_show') : t('providers.key_hide')}
        aria-label={revealed === null ? t('providers.key_show') : t('providers.key_hide')}
        disabled={busy}
        onclick={() => void (revealed === null ? reveal() : (revealed = null))}
        >{#if revealed === null}<EyeIcon />{:else}<EyeOffIcon />{/if}</Button
      >
    </div>
  </div>
  {#if askingPassword}<Input
      type="password"
      class="mt-2"
      placeholder={t('providers.key_current_password')}
      bind:value={password}
      onkeydown={(event) => event.key === 'Enter' && void reveal()}
    />{/if}
{/if}
