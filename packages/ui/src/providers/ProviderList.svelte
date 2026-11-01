<script lang="ts">
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis-vertical'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import SearchIcon from '@lucide/svelte/icons/search'
  import SquarePenIcon from '@lucide/svelte/icons/square-pen'
  import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal'
  import Trash2Icon from '@lucide/svelte/icons/trash-2'
  import UserPenIcon from '@lucide/svelte/icons/user-pen'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { Provider } from '../ports'
  import { cn } from '../utils'

  let {
    providers,
    selectedId,
    onselect,
    onaction,
    onadd,
  }: {
    providers: Provider[]
    selectedId: string | null
    onselect: (provider: Provider) => void
    onaction: (provider: Provider, action: 'edit' | 'notes' | 'delete') => void
    onadd: () => void
  } = $props()
  const { i18n } = getKitContext()
  const { t } = i18n
  const FILTERS = ['all', 'enabled', 'disabled'] as const
  let query = $state('')
  let filter = $state<(typeof FILTERS)[number]>('all')
  let menuFor = $state<string | null>(null)
  const visible = $derived(
    providers.filter(
      (p) =>
        p.name.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()) &&
        (filter === 'all' || (filter === 'enabled') === p.enabled),
    ),
  )
  function status(p: Provider): string | null {
    if (!p.enabled) return null
    return p.endpoints.length === 0 ||
      p.endpoints.some((e) => !e.keys.some((k) => k.enabled && k.readable))
      ? 'bg-warning'
      : 'bg-success'
  }
</script>

<nav class="flex w-full shrink-0 flex-col gap-1 sm:w-60" aria-label={t('providers.title')}>
  <div class="mb-1 flex items-center gap-1.5">
    <div class="relative min-w-0 flex-1">
      <SearchIcon
        class="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2"
      />
      <Input
        class="pl-8"
        placeholder={t('providers.search')}
        aria-label={t('providers.search')}
        bind:value={query}
        autocomplete="off"
      />
    </div>
    <Select.Root
      type="single"
      value={filter}
      onValueChange={(value) => (filter = value as typeof filter)}
    >
      <Select.Trigger
        class={cn('shrink-0 px-2 [&>svg:last-child]:hidden', filter !== 'all' && 'bg-accent')}
        aria-label={t('providers.filter')}
        title={t(`providers.filter_${filter}`)}
      >
        <SlidersHorizontalIcon />
      </Select.Trigger>
      <Select.Content
        ><Select.Group
          >{#each FILTERS as f (f)}<Select.Item
              value={f}
              label={t(`providers.filter_${f}`)}
            />{/each}</Select.Group
        ></Select.Content
      >
    </Select.Root>
  </div>
  <ul class="flex min-w-0 flex-col gap-0.5">
    {#each visible as p (p.id)}
      {@const dot = status(p)}
      <li
        class="group/row relative"
        oncontextmenu={(event) => {
          event.preventDefault()
          menuFor = p.id
        }}
      >
        <button
          type="button"
          class={cn(
            'hover:bg-muted flex h-9 w-full min-w-0 items-center gap-2.5 rounded-kit px-2 pr-8 text-left text-kit-base',
            selectedId === p.id && 'bg-muted font-medium',
          )}
          aria-current={selectedId === p.id ? 'true' : undefined}
          onclick={() => onselect(p)}
        >
          <span
            aria-hidden="true"
            class="bg-primary/10 text-primary flex size-6 shrink-0 items-center justify-center rounded-md text-kit-xs font-semibold"
            >{p.name.slice(0, 1).toUpperCase()}</span
          >
          <span class={cn('min-w-0 flex-1 truncate', !p.enabled && 'text-muted-foreground')}
            >{p.name}</span
          >
          {#if dot}<span class={cn('size-1.5 shrink-0 rounded-full', dot)}></span><span
              class="sr-only"
              >{t(
                dot === 'bg-success' ? 'providers.filter_enabled' : 'providers.needs_setup',
              )}</span
            >{/if}
        </button>
        <DropdownMenu.Root
          open={menuFor === p.id}
          onOpenChange={(open) => (menuFor = open ? p.id : null)}
        >
          <DropdownMenu.Trigger
            >{#snippet child({ props })}<button
                {...props}
                type="button"
                class={cn(
                  'text-muted-foreground hover:text-foreground hover:bg-accent absolute top-1/2 right-1 inline-flex size-6 -translate-y-1/2 items-center justify-center rounded-md opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100',
                  menuFor === p.id && 'opacity-100',
                )}
                aria-label={t('providers.row_menu', { name: p.name })}
                title={t('providers.row_menu', { name: p.name })}
                ><EllipsisIcon class="size-4" /></button
              >{/snippet}</DropdownMenu.Trigger
          >
          <DropdownMenu.Content align="start" class="min-w-36">
            <DropdownMenu.Item onSelect={() => onaction(p, 'edit')}
              ><SquarePenIcon />{t('providers.edit')}</DropdownMenu.Item
            >
            <DropdownMenu.Item onSelect={() => onaction(p, 'notes')}
              ><UserPenIcon />{t('providers.notes')}</DropdownMenu.Item
            >
            {#if !p.presetId}<DropdownMenu.Item
                variant="destructive"
                onSelect={() => onaction(p, 'delete')}
                ><Trash2Icon />{t('providers.remove')}</DropdownMenu.Item
              >{/if}
          </DropdownMenu.Content>
        </DropdownMenu.Root>
      </li>
    {:else}<li class="text-muted-foreground px-2 py-4 text-center text-kit-sm">
        {t('providers.no_matches')}
      </li>{/each}
  </ul>
  <Button variant="outline" class="mt-2" onclick={onadd}><PlusIcon />{t('providers.add')}</Button>
</nav>
