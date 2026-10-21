<script lang="ts">
  // Input with a hover/focus copy button at the inline end; shown only when non-empty.
  import CopyIcon from '@lucide/svelte/icons/copy'
  import type { ComponentProps } from 'svelte'
  import { Input } from '#lib/components/ui/input/index.js'
  import { cn } from '#lib/utils.js'
  import { getKitContext } from './context'

  let { value = $bindable(), class: className, ...rest }: ComponentProps<typeof Input> = $props()
  const { i18n, notify } = getKitContext()
  const { t } = i18n
  const text = $derived(String(value ?? '').trim())

  async function copy() {
    try {
      await navigator.clipboard.writeText(text)
      notify.toast('success', t('chat.copied'))
    } catch {
      notify.toast('error', t('chat.copyFailed'))
    }
  }
</script>

<div class={cn('group/copy relative min-w-0', className)}>
  <Input bind:value class={cn('w-full', text && 'pr-8')} {...rest} />
  {#if text}<button
      type="button"
      class="text-muted-foreground hover:text-foreground hover:bg-accent focus-visible:ring-ring/50 rounded-kit absolute top-1/2 right-1.5 inline-flex size-6 -translate-y-1/2 items-center justify-center opacity-0 transition-opacity outline-none group-focus-within/copy:opacity-100 group-hover/copy:opacity-100 focus-visible:opacity-100 focus-visible:ring-2"
      aria-label={t('chat.copy')}
      title={t('chat.copy')}
      onclick={() => void copy()}><CopyIcon class="size-3.5" /></button
    >{/if}
</div>
