<script lang="ts">
  // Key management (docs/34 §5.2): every action saves at once; one request at a time.
  import { untrack } from 'svelte'
  import CheckIcon from '@lucide/svelte/icons/check'
  import CopyIcon from '@lucide/svelte/icons/copy'
  import EditIcon from '@lucide/svelte/icons/pencil'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import TrashIcon from '@lucide/svelte/icons/trash-2'
  import XIcon from '@lucide/svelte/icons/x'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { getKitContext } from '../context'
  import type { Endpoint, EndpointKey, EndpointKeySave, Provider, ProvidersPort } from '../ports'
  import { keyError, maskedText } from './draft'

  const MAX_KEYS = 16

  let {
    open = $bindable(false),
    endpoint,
    editKeyId,
    onchanged,
  }: {
    open: boolean
    endpoint: Endpoint
    /** Key whose edit row is open when the dialog opens. */
    editKeyId: string | null
    onchanged: (providers: Provider[]) => void
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  /** `id: null` is the add row. */
  let edit = $state<{ id: string | null; label: string; key: string } | null>(null)
  let busy = $state(false)
  let password = $state('')
  /** Key waiting for the current password before it can be copied or edited (web). */
  let asking = $state<{ id: string; action: 'copy' | 'edit' } | null>(null)
  $effect.pre(() => {
    if (!open) return
    untrack(() => {
      edit = null
      password = ''
      asking = null
      const key = endpoint.keys.find((k) => k.id === editKeyId)
      if (key) void startEdit(key)
    })
  })
  const enabled = $derived(endpoint.keys.filter((k) => k.enabled).length)
  const name = (key: EndpointKey) => key.label || maskedText(key, t)

  async function run(action: (port: ProvidersPort) => Promise<Provider[]>): Promise<boolean> {
    if (!ports.providers || busy) return false
    busy = true
    try {
      onchanged(await action(ports.providers))
      return true
    } catch (error) {
      notify.toast('error', keyError(error, t))
      return false
    } finally {
      busy = false
    }
  }
  async function save() {
    if (!edit) return
    const key = edit.key.trim()
    if (edit.id === null && !key) return notify.toast('error', t('providers.keys_err_empty'))
    const input: EndpointKeySave = { endpointId: endpoint.id, label: edit.label.trim() }
    if (edit.id !== null) input.id = edit.id
    if (key) input.key = key
    if (await run((port) => port.saveKey(input))) edit = null
  }
  async function plaintext(key: EndpointKey, action: 'copy' | 'edit'): Promise<string | null> {
    const port = ports.providers
    if (!port || !key.readable || busy) return null
    if (port.revealNeedsPassword && !password) {
      asking = { id: key.id, action }
      return null
    }
    busy = true
    try {
      const text = await port.revealKey(key.id, password)
      asking = null
      return text
    } catch (error) {
      password = ''
      notify.toast('error', error instanceof Error ? error.message : t('providers.key_reveal_failed'))
      return null
    } finally {
      busy = false
    }
  }
  async function copy(key: EndpointKey) {
    const text = await plaintext(key, 'copy')
    if (text === null) return
    try {
      await navigator.clipboard.writeText(text)
      notify.toast('success', t('providers.key_copied'))
    } catch {
      notify.toast('error', t('providers.key_copy_failed'))
    }
  }
  /** Edits start from the stored key; an unreadable key can only be replaced. */
  async function startEdit(key: EndpointKey) {
    if (!key.readable) {
      edit = { id: key.id, label: key.label, key: '' }
      return
    }
    const text = await plaintext(key, 'edit')
    if (text !== null) edit = { id: key.id, label: key.label, key: text }
  }
  function keydown(event: KeyboardEvent) {
    if (event.key !== 'Enter') return
    event.preventDefault()
    void save()
  }
</script>

{#snippet editRow(isNew: boolean)}
  {#if edit}
    <div class="flex min-w-0 items-center gap-1.5 py-2">
      <div class="flex min-w-0 flex-1 flex-col gap-1.5">
        <Input
          class="h-8"
          maxlength={40}
          autocomplete="off"
          name="provider-api-key-label"
          placeholder={t('providers.keys_label')}
          aria-label={t('providers.keys_label')}
          disabled={busy}
          bind:value={edit.label}
          onkeydown={keydown}
        />
        <Input
          type="text"
          autocomplete="off"
          name="provider-api-key"
          spellcheck={false}
          class="h-8 font-mono [-webkit-text-security:disc]"
          maxlength={4096}
          autofocus
          placeholder={isNew ? t('providers.key_placeholder') : t('providers.keys_keep')}
          aria-label={t('providers.api_key')}
          disabled={busy}
          bind:value={edit.key}
          onkeydown={keydown}
        />
      </div>
      <Button
        variant="ghost"
        size="icon-xs"
        title={t('providers.keys_save')}
        aria-label={t('providers.keys_save')}
        disabled={busy}
        onclick={() => void save()}><CheckIcon /></Button
      >
      <Button
        variant="ghost"
        size="icon-xs"
        title={t('providers.keys_cancel')}
        aria-label={t('providers.keys_cancel')}
        disabled={busy}
        onclick={() => (edit = null)}><XIcon /></Button
      >
    </div>
  {/if}
{/snippet}

<Dialog.Root bind:open>
  <Dialog.Content
    class="flex max-h-[85dvh] min-w-0 flex-col sm:max-w-md"
    onEscapeKeydown={(event) => {
      // Escape cancels the edit row first; a second one closes the dialog.
      if (!edit) return
      event.preventDefault()
      edit = null
    }}
  >
    <Dialog.Header class="min-w-0 text-left">
      <Dialog.Title>{t('providers.keys_manage')}</Dialog.Title>
      <Dialog.Description>{t('providers.keys_desc')}</Dialog.Description>
    </Dialog.Header>
    {#if asking}
      <Input
        type="password"
        autocomplete="new-password"
        placeholder={t('providers.key_current_password')}
        aria-label={t('providers.key_current_password')}
        disabled={busy}
        bind:value={password}
        onkeydown={(event) => {
          const key = endpoint.keys.find((k) => k.id === asking?.id)
          if (event.key !== 'Enter' || !password || !key) return
          void (asking?.action === 'edit' ? startEdit(key) : copy(key))
        }}
      />
    {/if}
    <ul class="flex min-h-0 min-w-0 flex-col divide-y overflow-y-auto">
      {#each endpoint.keys as key (key.id)}
        <li class="min-w-0">
          {#if edit?.id === key.id}
            {@render editRow(false)}
          {:else}
            <div class="flex min-w-0 items-center gap-1 py-2">
              <button
                type="button"
                class="flex min-w-0 flex-1 flex-col text-left disabled:cursor-default"
                title={t('providers.key_copy')}
                disabled={busy || !key.readable}
                onclick={() => void copy(key)}
              >
                {#if key.label}<span class="truncate text-sm">{key.label}</span>{/if}
                <span class="truncate font-mono text-xs text-muted-foreground">{maskedText(key, t)}</span>
              </button>
              <Button
                variant="ghost"
                size="icon-xs"
                title={t('providers.key_copy')}
                aria-label={t('providers.key_copy')}
                disabled={busy || !key.readable}
                onclick={() => void copy(key)}><CopyIcon /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                title={t('providers.key_edit')}
                aria-label={t('providers.key_edit')}
                disabled={busy}
                onclick={() => void startEdit(key)}><EditIcon /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                class="hover:bg-destructive/10 hover:text-destructive"
                title={t('providers.key_delete')}
                aria-label={t('providers.key_delete')}
                disabled={busy}
                onclick={() => void run((port) => port.removeKey(endpoint.id, key.id))}><TrashIcon /></Button
              >
              <Switch
                size="sm"
                class="ml-1"
                aria-label={t('providers.keys_enable_for', { name: name(key) })}
                checked={key.enabled}
                disabled={busy}
                onCheckedChange={(next) =>
                  void run((port) => port.saveKey({ endpointId: endpoint.id, id: key.id, enabled: next }))}
              />
            </div>
          {/if}
        </li>
      {:else}
        {#if edit?.id !== null}
          <li class="text-muted-foreground py-6 text-center text-kit-sm">{t('providers.keys_empty')}</li>
        {/if}
      {/each}
      {#if edit?.id === null}<li class="min-w-0">{@render editRow(true)}</li>{/if}
    </ul>
    <Button
      variant="secondary"
      class="w-full"
      disabled={busy || edit !== null || endpoint.keys.length >= MAX_KEYS}
      onclick={() => (edit = { id: null, label: '', key: '' })}><PlusIcon />{t('providers.keys_add')}</Button
    >
    <p class="text-muted-foreground text-right text-kit-xs">
      {t('providers.keys_enabled_count', { enabled, total: endpoint.keys.length })}
    </p>
  </Dialog.Content>
</Dialog.Root>
