<script lang="ts">
  import BrainIcon from '@lucide/svelte/icons/brain'
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right'
  import ChevronsDownUpIcon from '@lucide/svelte/icons/chevrons-down-up'
  import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down'
  import EyeIcon from '@lucide/svelte/icons/eye'
  import Globe2Icon from '@lucide/svelte/icons/globe-2'
  import MinusIcon from '@lucide/svelte/icons/minus'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw'
  import SearchIcon from '@lucide/svelte/icons/search'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import WrenchIcon from '@lucide/svelte/icons/wrench'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { Model } from '../ports'
  import { cn } from '../utils'
  import { groupKey, type EndpointDraft } from './draft'
  import ModelSettingsDialog from './ModelSettingsDialog.svelte'

  let {
    endpoint,
    onsync,
    oncommit,
    usedBy,
    targetModel,
  }: {
    endpoint: EndpointDraft
    onsync: () => void
    oncommit: () => void
    usedBy?: ((endpointId: string, modelId: string) => string[]) | undefined
    targetModel?: Model | null
  } = $props()

  const { i18n } = getKitContext()
  const { t } = i18n
  const CAPS = ['vision', 'reasoning', 'tools', 'web'] as const
  type Cap = (typeof CAPS)[number]
  const CAP_STYLE: Record<Cap, string> = {
    vision: 'bg-success/15 text-success',
    reasoning: 'bg-info/15 text-info',
    tools: 'bg-warning/15 text-warning',
    web: 'bg-primary/10 text-primary',
  }
  let modelOpen = $state(false)
  let editing = $state<Model | null>(null)
  let openedTarget = $state<string | null>(null)
  $effect.pre(() => {
    if (targetModel && openedTarget !== targetModel.id) {
      openedTarget = targetModel.id
      editing = targetModel
      modelOpen = true
    }
  })
  let addOpen = $state(false)
  let newModelId = $state('')
  let searching = $state(false)
  let query = $state('')
  let capFilter = $state<Cap | 'all'>('all')
  let folded = $state<Record<string, boolean>>({})

  const has = (model: Model, cap: Cap) =>
    cap === 'vision'
      ? model.canVision
      : cap === 'reasoning'
        ? model.reasoning !== null
        : cap === 'tools'
          ? model.supportsTools === true
          : model.nativeWebSearch
  const shown = $derived.by(() => {
    const search = query.trim().toLowerCase()
    return endpoint.models.filter(
      (model) =>
        (model.id.toLowerCase().includes(search) || !!model.name?.toLowerCase().includes(search)) &&
        (capFilter === 'all' || has(model, capFilter)),
    )
  })
  const groups = $derived(Object.entries(Object.groupBy(shown, groupKey)))
  const allFolded = $derived(groups.length > 0 && groups.every(([group]) => folded[group]))

  function setModels(models: Model[]) {
    endpoint.models = models
    oncommit()
  }
  function addModel() {
    const id = newModelId.trim()
    if (id && !endpoint.models.some((model) => model.id === id)) {
      setModels([
        ...endpoint.models,
        {
          id,
          name: null,
          groupName: null,
          modelType: 'chat',
          typeSource: 'user',
          contextWindow: null,
          maxOutput: null,
          canVision: false,
          supportsTools: null,
          reasoning: null,
          nativeWebSearch: false,
          pricing: null,
        },
      ])
    }
    newModelId = ''
    addOpen = false
  }
</script>

<section class="flex min-w-0 flex-col gap-2" aria-labelledby="kit-endpoint-models">
  <div class="flex min-w-0 flex-wrap items-center gap-1">
    <h3 id="kit-endpoint-models" class="mr-1 text-kit-base font-semibold">
      {t('providers.models')}
    </h3>
    <Button
      variant="ghost"
      size="icon-xs"
      title={t(allFolded ? 'providers.expand_all' : 'providers.collapse_all')}
      aria-label={t(allFolded ? 'providers.expand_all' : 'providers.collapse_all')}
      disabled={groups.length === 0}
      onclick={() =>
        (folded = allFolded ? {} : Object.fromEntries(groups.map(([group]) => [group, true])))}
      >{#if allFolded}<ChevronsUpDownIcon />{:else}<ChevronsDownUpIcon />{/if}</Button
    >
    {#if searching || query}
      <Input
        class="h-7 w-36 text-kit-sm"
        autofocus
        placeholder={t('providers.model_search')}
        aria-label={t('providers.model_search')}
        bind:value={query}
        onblur={() => (searching = false)}
        onkeydown={(event) => event.key === 'Escape' && ((query = ''), event.currentTarget.blur())}
      />
    {:else}
      <Button
        variant="ghost"
        size="icon-xs"
        title={t('providers.model_search')}
        aria-label={t('providers.model_search')}
        onclick={() => (searching = true)}><SearchIcon /></Button
      >
    {/if}
    <Select.Root
      type="single"
      value={capFilter}
      onValueChange={(value) => (capFilter = value as Cap | 'all')}
    >
      <Select.Trigger
        size="sm"
        class={cn('border-0 shadow-none', capFilter !== 'all' && 'bg-accent')}
        aria-label={t('providers.cap_filter')}
      >
        {capFilter === 'all' ? t('providers.all_types') : t(`providers.cap_${capFilter}`)}
      </Select.Trigger>
      <Select.Content>
        <Select.Group>
          <Select.Item value="all" label={t('providers.all_types')} />
          {#each CAPS as cap (cap)}<Select.Item
              value={cap}
              label={t(`providers.cap_${cap}`)}
            />{/each}
        </Select.Group>
      </Select.Content>
    </Select.Root>
    <div class="ml-auto flex items-center gap-1">
      <Button variant="outline" size="sm" disabled={!endpoint.baseUrl} onclick={onsync}>
        <RefreshIcon />{t('providers.sync_models')}
      </Button>
      <Button
        variant="outline"
        size="icon-sm"
        title={t('providers.add_model_manual')}
        aria-label={t('providers.add_model_manual')}
        onclick={() => (addOpen = true)}><PlusIcon /></Button
      >
    </div>
  </div>

  {#each groups as [group, models = []] (group)}
    <section class="group/model-group overflow-hidden rounded-kit border">
      <div class="bg-muted/30 flex items-center gap-1 pr-2">
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-2 px-3 py-2 text-left text-kit-sm"
          aria-expanded={!folded[group]}
          onclick={() => (folded[group] = !folded[group])}
        >
          <ChevronRightIcon
            class={cn('size-4 shrink-0 transition-transform', !folded[group] && 'rotate-90')}
          />
          <span class="truncate">{group || t('providers.no_group')}</span>
          <span class="text-muted-foreground text-kit-xs tabular-nums">{models.length}</span>
        </button>
        <Button
          variant="ghost"
          size="icon-xs"
          class="opacity-0 group-hover/model-group:opacity-100 focus-visible:opacity-100"
          title={t('providers.remove_group')}
          aria-label={t('providers.remove_group')}
          onclick={() => setModels(endpoint.models.filter((model) => !models.includes(model)))}
          ><MinusIcon /></Button
        >
      </div>
      {#if !folded[group]}
        <ul class="divide-y border-t">
          {#each models as model (model.id)}
            <li class="hover:bg-accent/40 flex min-w-0 items-center gap-2 px-3 py-1.5">
              <span
                aria-hidden="true"
                class="bg-muted text-muted-foreground flex size-6 shrink-0 items-center justify-center rounded-full text-kit-xs uppercase"
                >{(model.name ?? model.id).slice(0, 1)}</span
              >
              <span class="min-w-0 flex-1 truncate text-kit-sm" title={model.id}
                >{model.name ?? model.id}</span
              >
              <span class="flex shrink-0 items-center gap-0.5">
                {#each CAPS.filter((cap) => has(model, cap)) as cap (cap)}
                  <span
                    class={cn(
                      'flex h-5 w-6 items-center justify-center rounded-full',
                      CAP_STYLE[cap],
                    )}
                    title={t(`providers.cap_${cap}`)}
                  >
                    {#if cap === 'vision'}<EyeIcon
                        class="size-3"
                      />{:else if cap === 'reasoning'}<BrainIcon
                        class="size-3"
                      />{:else if cap === 'tools'}<WrenchIcon class="size-3" />{:else}<Globe2Icon
                        class="size-3"
                      />{/if}
                    <span class="sr-only">{t(`providers.cap_${cap}`)}</span>
                  </span>
                {/each}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                title={t('providers.model_settings')}
                aria-label={t('providers.model_settings_for', { id: model.id })}
                onclick={() => {
                  editing = model
                  modelOpen = true
                }}><Settings2Icon /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                title={t('providers.remove_model', { id: model.id })}
                aria-label={t('providers.remove_model', { id: model.id })}
                onclick={() => setModels(endpoint.models.filter((item) => item.id !== model.id))}
                ><MinusIcon /></Button
              >
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {:else}
    <p
      class="text-muted-foreground rounded-kit border border-dashed px-3 py-6 text-center text-kit-sm"
    >
      {endpoint.models.length === 0 ? t('providers.no_models') : t('providers.no_model_matches')}
    </p>
  {/each}
</section>

<ModelSettingsDialog
  bind:open={modelOpen}
  model={editing}
  protocol={endpoint.protocol}
  usedBy={editing && endpoint.saved && usedBy ? usedBy(endpoint.saved.id, editing.id) : []}
  onsave={(saved) =>
    setModels(endpoint.models.map((model) => (model.id === saved.id ? saved : model)))}
/>

<Dialog.Root bind:open={addOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header><Dialog.Title>{t('providers.add_model_manual')}</Dialog.Title></Dialog.Header>
    <form
      class="contents"
      onsubmit={(event) => {
        event.preventDefault()
        addModel()
      }}
    >
      <Field.Field>
        <Field.Label for="kit-model-new">{t('providers.model_id')}</Field.Label>
        <Input
          id="kit-model-new"
          class="font-mono"
          autocomplete="off"
          spellcheck={false}
          bind:value={newModelId}
        />
      </Field.Field>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (addOpen = false)}
          >{t('common.cancel')}</Button
        >
        <Button type="submit" disabled={!newModelId.trim()}>{t('common.confirm')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
