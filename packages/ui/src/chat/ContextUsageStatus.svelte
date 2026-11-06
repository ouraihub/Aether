<script lang="ts">
  // Inline context status beside Send (docs/31 §4.3); opens the shared usage popover.
  import type { Snippet } from 'svelte'
  import { cn } from '#lib/utils.js'
  import type { KitI18n } from '../i18n'
  import type { ContextUsage } from '../ports'
  import UsageControl from './UsageControl.svelte'
  import PathIcon from './icons/PathIcon.svelte'
  import {
    detectPlatform,
    pressure,
    spending,
    summary as contextSummary,
    type Platform,
    type SpendingTurn,
  } from './context-status'
  import { applePath, linuxPath, wandPath, windowsPath } from './icons/status-icons'

  let {
    context,
    contextWindow = null,
    turns,
    modelName = '',
    i18n,
    platform = detectPlatform(),
    settings,
    details,
    open = $bindable(false),
  }: {
    context: ContextUsage | null
    /** Fallback capacity, e.g. the selected model's window, when the usage has none. */
    contextWindow?: number | null
    turns: readonly SpendingTurn[]
    modelName?: string
    i18n: KitI18n
    platform?: Platform
    settings?: Snippet | undefined
    details?: Snippet | undefined
    open?: boolean
  } = $props()
  const marks: Record<Platform, { name: string; path: string } | null> = {
    windows: { name: 'Windows', path: windowsPath },
    macos: { name: 'macOS', path: applePath },
    linux: { name: 'Linux', path: linuxPath },
    other: null,
  }
  const capacity = $derived(context?.contextWindow ?? contextWindow)
  const text = $derived(contextSummary(context, capacity))
  const spent = $derived(spending(turns))
  const level = $derived(pressure(context, capacity))
  const mark = $derived(marks[platform])
  const label = $derived(
    i18n
      .t('chat.status.label', { platform: mark?.name ?? '', summary: text, spending: spent })
      .trim(),
  )
</script>

<UsageControl
  bind:open
  {i18n}
  {settings}
  {details}
  summaryLabel={label}
  context={{
    tokens: context?.tokens ?? null,
    contextWindow: capacity,
    modelName,
    source: context?.source ?? 'unknown',
  }}
>
  {#snippet summary()}
    <span
      class={cn(
        'inline-flex min-w-0 items-center gap-1.5 font-mono text-xs tabular-nums',
        level === 'normal' && 'text-muted-foreground',
        level === 'warning' && 'text-warning',
        level === 'high' &&
          'text-[color-mix(in_oklch,var(--color-warning)_45%,var(--color-destructive))]',
        level === 'critical' && 'text-destructive',
      )}
      data-pressure={level}
    >
      {#if mark}<PathIcon
          data-composer-capacity
          d={mark.path}
          class="size-3 shrink-0 text-foreground"
        />{/if}
      <span class="shrink-0 whitespace-nowrap"
        >{text.split('/')[0]}<span data-composer-capacity>/{text.split('/')[1]}</span></span
      >
      <PathIcon d={wandPath} class="size-3 shrink-0" />
      <span data-composer-capacity class="mx-1.5 shrink-0 text-border" aria-hidden="true">·</span>
      <span data-composer-capacity class="min-w-0 truncate text-muted-foreground">{spent}</span>
    </span>
  {/snippet}
</UsageControl>
