<script lang="ts">
  // Effort section of the model picker: rail, level toggles, reset and hint (docs/31 §4.2).
  import { ToggleGroup } from 'bits-ui'
  import { Button } from '#lib/components/ui/button/index.js'
  import type { KitI18n } from '../i18n'
  import type { ReasoningEffort } from '../ports'
  import ReasoningSlider from './ReasoningSlider.svelte'
  import type { PickerModel } from './model-effort'

  let {
    model,
    effort,
    disabled = false,
    i18n,
    onselect,
  }: {
    model: PickerModel
    effort: ReasoningEffort
    disabled?: boolean
    i18n: KitI18n
    onselect: (effort: ReasoningEffort) => void
  } = $props()
  const t = (key: string, params?: Record<string, string | number>) => i18n.t(key, params)
  const label = $derived(t(`chat.effort.${effort}`))
</script>

<section class="flex flex-col gap-0.5 px-2 pt-2 pb-1" aria-label={t('chat.reasoningEffort')}>
  <div class="flex items-center justify-between gap-2">
    <p class="text-kit-xs text-muted-foreground">
      {t('chat.picker.effortFor', { name: model.name })}
    </p>
    {#if effort !== 'default'}
      <Button variant="link" size="xs" {disabled} onclick={() => onselect('default')}>
        {t('chat.resetReasoning')}
      </Button>
    {/if}
  </div>
  {#if model.efforts.length > 1}
    <ReasoningSlider
      efforts={model.efforts}
      value={effort}
      defaultIndex={Math.floor(model.efforts.length / 2)}
      {disabled}
      label={t('chat.reasoningEffort')}
      valueText={label}
      onchange={onselect}
    />
  {/if}
  <ToggleGroup.Root
    type="single"
    value={effort === 'default' ? '' : effort}
    onValueChange={(next) => next && onselect(next as ReasoningEffort)}
    {disabled}
    aria-label={t('chat.reasoningEffort')}
    class="flex"
  >
    {#each model.efforts as level (level)}
      <ToggleGroup.Item
        value={level}
        aria-label={`${t('chat.reasoningEffort')} ${t(`chat.effort.${level}`)}`}
        class="min-w-0 flex-1 rounded-kit-sm py-0.5 text-kit-xs whitespace-nowrap
          text-muted-foreground hover:text-foreground focus-visible:outline-2
          focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50
          data-[state=on]:font-semibold data-[state=on]:text-foreground"
      >
        {t(`chat.effort.${level}`)}
      </ToggleGroup.Item>
    {/each}
  </ToggleGroup.Root>
  <p class="text-kit-xs text-muted-foreground">{t(`chat.effortHint.${effort}`)}</p>
</section>
