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

<div class="flex items-center gap-2">
  {#if value === ''}
    <span class="text-muted-foreground flex-1 text-kit-sm">{t('providers.key_cleared')}</span>
    <Button variant="outline" size="sm" onclick={reset}>{t('providers.key_undo')}</Button>
  {:else if editing}
    <Input
      {id}
      type="password"
      autocomplete="off"
      class="flex-1"
      value={text}
      oninput={(event) => input(event.currentTarget.value)}
    />
    {#if replacing}
      <Button variant="outline" size="sm" onclick={reset}>{t('common.cancel')}</Button>
    {/if}
  {:else}
    <span {id} class="flex-1 font-mono text-kit-sm">
      {saved?.keyReadable ? saved.keyMasked : t('providers.key_unreadable')}
    </span>
    <Button variant="outline" size="sm" onclick={() => (replacing = true)}>{t('providers.key_replace')}</Button>
    <Button variant="outline" size="sm" onclick={() => (value = '')}>{t('providers.key_clear')}</Button>
  {/if}
</div>
