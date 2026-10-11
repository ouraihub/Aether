<script lang="ts">
  // Pick fetched models by group, select all, change type (docs/16 §4.1). Portals to body like every kit dialog.
  import { Button } from '#lib/components/ui/button/index.js'
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

  // Seed from the current selection each time a new result arrives.
  $effect.pre(() => {
    if (!result) return
    const current = Object.fromEntries(selected.map((m) => [m.id, m]))
    checked = Object.fromEntries(result.models.map((m) => [m.id, m.id in current]))
    types = Object.fromEntries(result.models.map((m) => [m.id, current[m.id]?.modelType ?? m.modelType]))
  })

  const models = $derived(result?.models ?? [])
  const groups = $derived(Object.entries(Object.groupBy(models, (m) => m.group ?? '')))
  const count = $derived(models.filter((m) => checked[m.id]).length)

  function setAll(on: boolean) {
    checked = Object.fromEntries(models.map((m) => [m.id, on]))
  }

  function apply() {
    onapply(
      models
        .filter((m) => checked[m.id])
        .map((m): Model => {
          const modelType = types[m.id] ?? m.modelType
          return {
            id: m.id,
            modelType,
            typeSource: modelType === m.modelType ? m.typeSource : 'user',
            contextWindow: m.contextWindow,
            maxOutput: m.maxOutput,
            canVision: m.canVision,
            supportsTools: m.supportsTools,
          }
        }),
    )
    open = false
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[80vh] flex-col sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t('providers.picker_title')}</Dialog.Title>
      <Dialog.Description>
        {t('providers.picker_desc', { count: models.length, url: result?.probeUrl ?? '' })}
      </Dialog.Description>
    </Dialog.Header>
    <label class="flex items-center gap-2 text-sm">
      <input
        type="checkbox"
        class="accent-primary"
        checked={models.length > 0 && count === models.length}
        onchange={(event) => setAll(event.currentTarget.checked)}
      />
      {t('providers.select_all')}
    </label>
    <div class="flex min-h-0 flex-col gap-3 overflow-y-auto">
      {#each groups as [group, items = []] (group)}
        <section class="flex flex-col gap-1">
          <h3 class="text-muted-foreground text-xs font-medium">{group || t('providers.no_group')}</h3>
          {#each items as model (model.id)}
            <div class="flex items-center gap-2">
              <label class="flex flex-1 items-center gap-2 truncate text-sm">
                <input type="checkbox" class="accent-primary" bind:checked={checked[model.id]} />
                <span class="truncate font-mono">{model.id}</span>
              </label>
              <Select.Root
                type="single"
                value={types[model.id] ?? model.modelType}
                onValueChange={(value) => (types[model.id] = value as ModelType)}
              >
                <Select.Trigger size="sm" aria-label={t('providers.model_type')}>
                  {t(`providers.type.${types[model.id] ?? model.modelType}`)}
                </Select.Trigger>
                <Select.Content>
                  <Select.Group>
                    {#each TYPES as type (type)}
                      <Select.Item value={type} label={t(`providers.type.${type}`)} />
                    {/each}
                  </Select.Group>
                </Select.Content>
              </Select.Root>
            </div>
          {/each}
        </section>
      {/each}
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{t('common.cancel')}</Button>
      <Button onclick={apply}>{t('providers.apply', { count })}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
