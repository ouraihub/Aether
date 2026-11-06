<script lang="ts">
  // Composer pill + upward popover (docs/31 §4.2). The host owns data and every event's effect.
  import { Command, Popover } from 'bits-ui'
  import CheckIcon from '@lucide/svelte/icons/check'
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right'
  import KeyIcon from '@lucide/svelte/icons/key-round'
  import PinIcon from '@lucide/svelte/icons/pin'
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw'
  import SearchIcon from '@lucide/svelte/icons/search'
  import { SvelteSet } from 'svelte/reactivity'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Separator } from '#lib/components/ui/separator/index.js'
  import { cn } from '#lib/utils.js'
  import type { KitI18n } from '../i18n'
  import type { ReasoningEffort } from '../ports'
  import { REASONING_EFFORTS } from '../providers/reasoning'
  import PickerEffort from './PickerEffort.svelte'
  import {
    effortBars,
    effortTone,
    groupModels,
    groupSummary,
    nextValue,
    type PickerModel,
    type PickerValue,
  } from './model-effort'

  let {
    models,
    value = null,
    defaultValue = null,
    disabled = false,
    refreshing = false,
    notice = '',
    i18n,
    open = $bindable(false),
    onselect,
    onsetdefault,
    onrefresh,
    onopensettings,
  }: {
    models: readonly PickerModel[]
    value?: PickerValue | null
    defaultValue?: PickerValue | null
    disabled?: boolean
    refreshing?: boolean
    notice?: string
    i18n: KitI18n
    open?: boolean
    onselect: (value: PickerValue) => void
    onsetdefault?: ((value: PickerValue) => void) | undefined
    onrefresh?: ((force: boolean) => void) | undefined
    /** Opens host settings for the current model when it declares no reasoning levels. */
    onopensettings?: (() => void) | undefined
  } = $props()
  const t = (key: string, params?: Record<string, string | number>) => i18n.t(key, params)
  let query = $state('')
  // Command tracks the highlighted item; arrows/Enter come from the primitive.
  let highlighted = $state('')
  let trigger = $state<HTMLButtonElement | null>(null)
  // Groups opened while browsing, and groups folded while searching (searching opens all).
  const expanded = new SvelteSet<string>()
  const searchFolded = new SvelteSet<string>()
  const searching = $derived(query.trim() !== '')
  // Hosts may declare levels in any order; the rail always reads 关闭 → 最大.
  const current = $derived.by(() => {
    const found = models.find((model) => model.key === value?.key)
    return found
      ? {
          ...found,
          efforts: REASONING_EFFORTS.filter((level) => found.efforts.includes(level)),
        }
      : null
  })
  const effort: ReasoningEffort = $derived(
    current && value && current.efforts.includes(value.effort) ? value.effort : 'default',
  )
  const effortLabel = $derived(t(`chat.effort.${effort}`))
  const name = $derived(current?.name ?? t('chat.picker.choose'))
  const bars = $derived(current ? effortBars(current, effort) : 0)
  const tone = $derived(effortTone(effort))
  const groups = $derived(groupModels(models, query))
  const isOpen = (id: string) => (searching ? !searchFolded.has(id) : expanded.has(id))
  const sameAsDefault = $derived(
    value !== null && value.key === defaultValue?.key && value.effort === defaultValue.effort,
  )
  $effect(() => {
    // A new current model reveals its group; other groups keep the user's folding.
    if (current) expanded.add(current.providerId)
  })
  function openChanged(next: boolean) {
    if (!next) return
    query = ''
    highlighted = value?.key ?? ''
    searchFolded.clear()
    onrefresh?.(false)
  }
  function toggle(id: string) {
    const set = searching ? searchFolded : expanded
    if (!set.delete(id)) set.add(id)
  }
</script>

<Popover.Root bind:open onOpenChange={openChanged}>
  <Popover.Trigger {disabled}>
    {#snippet child({ props })}
      <Button
        {...props}
        bind:ref={trigger}
        variant="ghost"
        size="sm"
        class="min-w-0 max-w-70 shrink"
        data-composer-model
        aria-label={t('chat.picker.trigger', { name, effort: effortLabel })}
      >
        <span
          data-composer-model-detail
          class="text-kit-xs text-muted-foreground"
          aria-hidden="true">{current?.provider.charAt(0).toUpperCase() ?? '·'}</span
        >
        <span class="min-w-0 truncate">{name}</span>
        {#if current && effort !== 'default'}
          <span class="inline-flex h-3 shrink-0 items-end gap-0.5" aria-hidden="true">
            {#each ['h-1', 'h-1.5', 'h-2', 'h-3'] as height, bar (height)}
              <span
                class={cn(
                  'w-0.5 rounded-full bg-border',
                  height,
                  bar < bars && tone === 'fast' && 'bg-info',
                  bar < bars && tone === 'regular' && 'bg-primary',
                  bar < bars && tone === 'deep' && 'bg-warning',
                )}
              ></span>
            {/each}
          </span>
          <span data-composer-model-detail class="text-kit-xs text-muted-foreground"
            >{effortLabel}</span
          >
        {/if}
        {#if current?.hasKey}<KeyIcon
            data-composer-model-detail
            data-icon="inline-end"
            aria-hidden="true"
          />{/if}
        <ChevronDownIcon data-icon="inline-end" aria-hidden="true" />
      </Button>
    {/snippet}
  </Popover.Trigger>
  <Popover.Portal>
    <Popover.Content
      class={cn(
        'z-[210] flex w-[min(360px,calc(100vw-16px))] flex-col overflow-hidden rounded-kit',
        'max-h-[min(calc(100dvh-64px),var(--bits-popover-content-available-height))]',
        'border bg-popover text-xs text-popover-foreground shadow-lg',
      )}
      side="top"
      align="start"
      sideOffset={4}
      collisionPadding={8}
      trapFocus={false}
      preventScroll={false}
      onEscapeKeydown={(event) => {
        if (event.isComposing) event.preventDefault()
        event.stopPropagation()
      }}
      onCloseAutoFocus={(event) => {
        event.preventDefault()
        trigger?.focus({ preventScroll: true })
      }}
    >
      <Command.Root
        bind:value={highlighted}
        shouldFilter={false}
        loop
        label={t('chat.picker.choose')}
        class="flex min-h-0 flex-col"
      >
        <div class="flex items-center gap-1.5 px-2 text-muted-foreground">
          <SearchIcon class="size-3.5 shrink-0" aria-hidden="true" />
          <Command.Input
            bind:value={query}
            placeholder={t('chat.picker.search')}
            aria-label={t('chat.picker.searchLabel')}
            class="h-8 min-w-0 flex-1 bg-transparent text-foreground outline-none"
          />
        </div>
        <Separator />
        <Command.List class="max-h-[clamp(48px,calc(100dvh-250px),248px)] overflow-y-auto p-1">
          {#if models.length === 0 || (searching && groups.length === 0)}
            <Command.Empty forceMount class="p-3 text-kit-xs text-muted-foreground">
              {t(models.length === 0 ? 'chat.picker.empty' : 'chat.picker.noMatch')}
            </Command.Empty>
          {/if}
          {#each groups as group (group.id)}
            {@const shown = isOpen(group.id)}
            <Command.Group value={`group\n${group.id}`}>
              <Command.GroupHeading>
                <Button
                  variant="ghost"
                  size="xs"
                  class="w-full justify-start font-normal"
                  aria-expanded={shown}
                  aria-label={t('chat.picker.group', {
                    provider: group.provider,
                    state: t(shown ? 'chat.picker.expanded' : 'chat.picker.collapsed'),
                  })}
                  onclick={() => toggle(group.id)}
                >
                  <span class="text-muted-foreground">{group.provider.charAt(0).toUpperCase()}</span
                  >
                  <span class="min-w-0 truncate">{group.provider}</span>
                  <span class="ml-auto max-w-32 truncate text-muted-foreground"
                    >{groupSummary(group, i18n)}</span
                  >
                  {#if shown}<ChevronDownIcon data-icon="inline-end" aria-hidden="true" />{:else}
                    <ChevronRightIcon data-icon="inline-end" aria-hidden="true" />{/if}
                </Button>
              </Command.GroupHeading>
              {#if shown}
                <Command.GroupItems>
                  {#each group.models as model (model.key)}
                    {@const selected = model.key === value?.key}
                    <Command.Item
                      value={model.key}
                      {disabled}
                      aria-current={selected ? 'true' : undefined}
                      aria-label={t('chat.picker.select', {
                        name: model.name,
                        provider: model.provider,
                      })}
                      onSelect={() => !disabled && onselect(nextValue(model, value, query))}
                      class={cn(
                        'flex cursor-pointer items-center gap-2 rounded-kit-sm py-1 pr-2 pl-6',
                        'data-[selected]:bg-accent data-[selected]:text-accent-foreground',
                        'data-[disabled]:cursor-not-allowed data-[disabled]:opacity-50',
                        selected && 'bg-accent/60',
                      )}
                    >
                      <span class="flex min-w-0 flex-1 flex-col">
                        <span class="truncate">{model.name}</span>
                        <span
                          class="flex min-w-0 items-center gap-1 text-kit-xs text-muted-foreground"
                        >
                          {#if model.hasKey}<KeyIcon
                              class="size-3 shrink-0"
                              aria-hidden="true"
                            />{/if}
                          <span class="truncate">{model.detail}</span>
                        </span>
                      </span>
                      {#if selected}<CheckIcon class="size-3 shrink-0" aria-hidden="true" />{/if}
                    </Command.Item>
                  {/each}
                </Command.GroupItems>
              {/if}
            </Command.Group>
          {/each}
        </Command.List>
      </Command.Root>
      {#if current && current.efforts.length > 0}
        <Separator />
        <PickerEffort
          model={current}
          {effort}
          {disabled}
          {i18n}
          onselect={(next) => onselect({ key: current.key, effort: next })}
        />
      {:else if current}
        <Separator />
        <div class="flex items-center justify-between gap-2 px-2 py-1.5">
          <p class="text-kit-xs text-muted-foreground">{t('chat.reasoningUnconfigured')}</p>
          {#if onopensettings}
            <Button
              variant="outline"
              size="xs"
              onclick={() => {
                open = false
                onopensettings()
              }}>{t('chat.openSettings')}</Button
            >
          {/if}
        </div>
      {/if}
      {#if notice}
        <p class="truncate px-2 pb-1 text-kit-xs text-muted-foreground" role="status">
          {notice}
        </p>
      {/if}
      <Separator />
      <div class="flex items-center gap-0.5 p-1">
        {#if onsetdefault}
          <Button
            variant="ghost"
            size="xs"
            disabled={disabled || value === null || sameAsDefault}
            onclick={() => value && onsetdefault(value)}
          >
            <PinIcon data-icon="inline-start" aria-hidden="true" />{t('chat.picker.setDefault')}
          </Button>
        {/if}
        <Button
          variant="ghost"
          size="xs"
          disabled={disabled || !current?.efforts.includes('none')}
          onclick={() => current && onselect({ key: current.key, effort: 'none' })}
        >
          <RefreshIcon data-icon="inline-start" aria-hidden="true" />{t('chat.picker.resetOff')}
        </Button>
        {#if onrefresh}
          <Button
            variant="outline"
            size="xs"
            class="ml-auto"
            disabled={refreshing}
            onclick={() => onrefresh(true)}
          >
            <RefreshIcon data-icon="inline-start" aria-hidden="true" />
            {t(refreshing ? 'chat.picker.refreshing' : 'chat.picker.refresh')}
          </Button>
        {/if}
      </div>
    </Popover.Content>
  </Popover.Portal>
</Popover.Root>
