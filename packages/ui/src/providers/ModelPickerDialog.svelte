<script lang="ts">
  // Search and filter fetched models; +/- changes the draft until Apply.
  import { untrack } from 'svelte'
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import Plus from '@lucide/svelte/icons/plus'
  import Minus from '@lucide/svelte/icons/minus'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { Model, ModelType, ProbeResult } from '../ports'

  let {
    open = $bindable(false),
    result,
    selected,
    onapply,
  }: {
    open: boolean
    result: ProbeResult | null
    /** Currently selected models; preselected and their user-set types kept. */
    selected: Model[]
    onapply: (models: Model[]) => void
  } = $props()

  const TYPES: ModelType[] = ['chat', 'image', 'video', 'tts', 'stt', 'embedding', 'rerank', 'unknown']
  const { t } = getKitContext().i18n

  let checked = $state<Record<string, boolean>>({})
  let types = $state<Record<string, ModelType>>({})
  let query = $state('')
  let filter = $state<ModelType | 'all'>('all')
  let collapsed = $state<Record<string, boolean>>({})

  $effect.pre(() => {
    if (!open || !result) return
    const current = Object.fromEntries(untrack(() => selected).map((m) => [m.id, m]))
    checked = Object.fromEntries(result.models.map((m) => [m.id, m.id in current]))
    types = Object.fromEntries(result.models.map((m) => [m.id, current[m.id]?.modelType ?? m.modelType]))
    query = ''
    filter = 'all'
    collapsed = {}
  })

  const models = $derived(result?.models ?? [])
  const searched = $derived(models.filter((m) => m.id.toLowerCase().includes(query.trim().toLowerCase())))
  const shown = $derived(searched.filter((m) => filter === 'all' || (types[m.id] ?? m.modelType) === filter))
  const groups = $derived(Object.entries(Object.groupBy(shown, (m) => m.group ?? '')))
  const count = $derived(models.filter((m) => checked[m.id]).length)
  const bulkRemoves = $derived(shown.length > 0 && shown.every((m) => checked[m.id]))

  function bulk() {
    const on = !bulkRemoves
    for (const model of shown) checked[model.id] = on
  }

  function apply() {
    onapply(
      models
        .filter((m) => checked[m.id])
        .map((m): Model => {
          const existing = selected.find((item) => item.id === m.id)
          const modelType = types[m.id] ?? m.modelType
          // Keep user-edited metadata (including reasoning/context) when a probe refreshes it.
          return {
            id: m.id,
            modelType,
            typeSource:
              existing?.modelType === modelType
                ? existing.typeSource
                : modelType === m.modelType
                  ? m.typeSource
                  : 'user',
            contextWindow: existing ? existing.contextWindow : m.contextWindow,
            maxOutput: existing ? existing.maxOutput : m.maxOutput,
            reasoning: existing ? existing.reasoning : m.reasoning,
            nativeWebSearch: modelType === 'chat' && (existing ? existing.nativeWebSearch : m.nativeWebSearch),
            canVision: existing ? existing.canVision : m.canVision,
            supportsTools: m.supportsTools,
          }
        }),
    )
    open = false
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85dvh] min-w-0 flex-col gap-3 sm:max-w-2xl">
    <Dialog.Header class="shrink-0 text-left">
      <Dialog.Title class="flex items-center gap-2 pr-6">
        {t('providers.picker_title')}
        <span aria-hidden="true" class="bg-muted rounded px-1.5 text-kit-xs tabular-nums">{models.length}</span>
      </Dialog.Title>
      <Dialog.Description>{t('providers.picker_hint')}</Dialog.Description>
    </Dialog.Header>
    <div class="flex shrink-0 items-center gap-2">
      <Input
        class="min-w-0 flex-1"
        bind:value={query}
        placeholder={t('providers.search_models')}
        aria-label={t('providers.search_models')}
        autocomplete="off"
        spellcheck={false}
      />
      <Button variant="outline" size="sm" class="shrink-0" disabled={shown.length === 0} onclick={bulk}>
        {t(bulkRemoves ? 'providers.remove_all' : 'providers.add_all')}
      </Button>
    </div>
    <div class="flex shrink-0 gap-1 overflow-x-auto pb-1" role="group" aria-label={t('providers.model_type')}>
      {#each ['all', ...TYPES] as type (type)}
        {@const hits =
          type === 'all' ? searched.length : searched.filter((m) => (types[m.id] ?? m.modelType) === type).length}
        <Button
          variant={filter === type ? 'secondary' : 'ghost'}
          size="sm"
          class="shrink-0"
          aria-pressed={filter === type}
          disabled={hits === 0 && type !== 'all'}
          onclick={() => (filter = type as ModelType | 'all')}
        >
          {type === 'all' ? t('providers.all_types') : t(`providers.type.${type as ModelType}`)}
          <span class="text-muted-foreground tabular-nums">{hits}</span>
        </Button>
      {/each}
    </div>
    <div class="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto" role="region" aria-label={t('providers.models')}>
      {#if shown.length === 0}
        <p class="text-muted-foreground py-8 text-center text-kit-sm">{t('providers.no_model_matches')}</p>
      {/if}
      {#each groups as [group, items = []] (group)}
        {@const folded = !query.trim() && collapsed[group]}
        <section class="flex min-w-0 flex-col">
          <Button
            variant="ghost"
            size="sm"
            class="justify-start"
            aria-expanded={!folded}
            onclick={() => (collapsed[group] = !collapsed[group])}
          >
            <ChevronRight class={folded ? '' : 'rotate-90'} />
            {group || t('providers.no_group')}
            <span class="bg-muted rounded px-1.5 tabular-nums">{items.length}</span>
          </Button>
          {#if !folded}
            <ul>
              {#each items as model (model.id)}
                <li class="hover:bg-accent/50 flex min-w-0 items-center gap-1 rounded-md py-0.5 pl-4">
                  <span class="min-w-0 flex-1 truncate font-mono text-kit-sm" title={model.id}>{model.id}</span>
                  <Select.Root
                    type="single"
                    value={types[model.id] ?? model.modelType}
                    onValueChange={(value) => (types[model.id] = value as ModelType)}
                  >
                    <Select.Trigger
                      size="sm"
                      class="shrink-0 border-0 bg-transparent shadow-none"
                      aria-label={t('providers.type_for', { id: model.id })}
                    >
                      {t(`providers.type.${types[model.id] ?? model.modelType}`)}
                    </Select.Trigger>
                    <Select.Content
                      ><Select.Group>
                        {#each TYPES as type (type)}
                          <Select.Item value={type} label={t(`providers.type.${type}`)} />
                        {/each}
                      </Select.Group></Select.Content
                    >
                  </Select.Root>
                  {#if model.canVision}<span class="text-muted-foreground hidden shrink-0 text-kit-xs sm:inline"
                      >{t('providers.vision')}</span
                    >{/if}
                  <span class="text-chart-2 w-8 shrink-0 text-right text-kit-xs"
                    >{checked[model.id] ? t('providers.picked') : ''}</span
                  >
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    class="shrink-0"
                    aria-label={t(checked[model.id] ? 'providers.remove_model' : 'providers.add_model', {
                      id: model.id,
                    })}
                    onclick={() => (checked[model.id] = !checked[model.id])}
                  >
                    {#if checked[model.id]}<Minus />{:else}<Plus />{/if}
                  </Button>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      {/each}
    </div>
    <Dialog.Footer class="shrink-0 flex-row flex-wrap items-center gap-2 sm:justify-between">
      <p class="text-muted-foreground mr-auto text-kit-xs" aria-live="polite">
        {t('providers.picked_count', { count })}
      </p>
      <Button variant="outline" size="sm" onclick={() => (open = false)}>{t('common.cancel')}</Button>
      <Button size="sm" onclick={apply}>{t('providers.apply', { count })}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
