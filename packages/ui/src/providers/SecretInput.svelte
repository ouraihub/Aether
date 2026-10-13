<script lang="ts">
  // Write-only key field: shows the masked stored value with explicit Replace / Clear (docs/16 §4.1, docs/11 陷阱 2).
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import type { Endpoint } from '../ports'

  let {
    id,
    saved,
    value = $bindable(),
  }: {
    id: string
    /** Stored endpoint, `null` for a new one. */
    saved: Endpoint | null
    /** `null` = keep, `''` = clear, else the new key. */
    value: string | null
  } = $props()

  const { t } = getKitContext().i18n
  let replacing = $state(false)
  let text = $state('')
  const editing = $derived(saved === null || !saved.hasKey || replacing)

  function input(next: string) {
    text = next
    // An emptied box keeps the stored key; only the Clear button clears it.
    value = next === '' ? null : next
  }

  function reset() {
    replacing = false
    text = ''
    value = null
  }
</script>

<div class="flex min-w-0 items-center gap-2">
  {#if value === ''}
    <span class="text-muted-foreground min-w-0 flex-1 text-kit-sm">{t('providers.key_cleared')}</span>
    <Button class="shrink-0" variant="outline" size="sm" onclick={reset}>{t('providers.key_undo')}</Button>
  {:else if editing}
    <Input
      {id}
      type="password"
      autocomplete="off"
      class="min-w-0 flex-1"
      value={text}
      oninput={(event) => input(event.currentTarget.value)}
    />
    {#if replacing}
      <Button class="shrink-0" variant="outline" size="sm" onclick={reset}>{t('common.cancel')}</Button>
    {/if}
  {:else}
    {@const shown = saved?.keyReadable ? (saved.keyMasked ?? '') : t('providers.key_unreadable')}
    <!-- Long masks shrink in the middle so the head/tail stay legible and the buttons never overflow. -->
    <span {id} class="flex min-w-0 flex-1 whitespace-nowrap font-mono text-kit-sm">
      {#if saved?.keyReadable && shown.length > 8}
        <span class="sr-only">{shown}</span>
        <span aria-hidden="true" class="shrink-0">{shown.slice(0, 4)}</span>
        <span aria-hidden="true" class="min-w-0 truncate">{shown.slice(4, -4)}</span>
        <span aria-hidden="true" class="shrink-0">{shown.slice(-4)}</span>
      {:else}
        <span class="truncate">{shown}</span>
      {/if}
    </span>
    <Button class="shrink-0" variant="outline" size="sm" onclick={() => (replacing = true)}
      >{t('providers.key_replace')}</Button
    >
    <Button class="shrink-0" variant="outline" size="sm" onclick={() => (value = '')}>{t('providers.key_clear')}</Button
    >
  {/if}
</div>
