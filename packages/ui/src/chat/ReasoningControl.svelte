<script lang="ts">
  import GaugeIcon from '@lucide/svelte/icons/gauge'
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import { Popover } from 'bits-ui'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import type { KitI18n } from '../i18n'
  import type { ReasoningEffort } from '../ports'
  import { REASONING_EFFORTS } from '../providers/reasoning'
  import ReasoningSlider from './ReasoningSlider.svelte'

  let {
    value = 'default',
    efforts,
    disabled = false,
    open = $bindable(false),
    i18n,
    onchange,
    onOpenSettings,
  }: {
    value?: ReasoningEffort
    efforts: readonly ReasoningEffort[]
    disabled?: boolean
    open?: boolean
    i18n: KitI18n
    onchange: (effort: ReasoningEffort) => void
    onOpenSettings?: (() => void) | undefined
  } = $props()
  const id = $props.id()
  let trigger = $state<HTMLButtonElement | null>(null)
  const t = (key: string) => i18n.t(key)
  const allowedEfforts = $derived(REASONING_EFFORTS.filter((effort) => efforts.includes(effort)))
  const effectiveEffort = $derived(
    value === 'default' || allowedEfforts.includes(value) ? value : 'default',
  )
  const effortLabel = $derived(t(`chat.effort.${effectiveEffort}`))
  const effortIndex = $derived(
    effectiveEffort === 'default'
      ? Math.floor(allowedEfforts.length / 2)
      : allowedEfforts.indexOf(effectiveEffort),
  )
  function choose(effort: ReasoningEffort) {
    if (!disabled && (effort === 'default' || allowedEfforts.includes(effort))) onchange(effort)
  }
</script>

<Popover.Root bind:open>
  <Popover.Trigger {disabled}>
    {#snippet child({ props })}
      <Button
        {...props}
        bind:ref={trigger}
        variant="ghost"
        size="sm"
        aria-label={`${t('chat.reasoningEffort')}: ${effortLabel}`}
      >
        <GaugeIcon data-icon="inline-start" />
        {effortLabel}
        <ChevronDownIcon data-icon="inline-end" />
      </Button>
    {/snippet}
  </Popover.Trigger>
  <Popover.Portal>
    <Popover.Content
      class="reasoning-popover"
      side="top"
      align="end"
      sideOffset={8}
      collisionPadding={8}
      trapFocus={false}
      preventScroll={false}
      aria-label={t('chat.reasoningEffort')}
      onEscapeKeydown={(event) => {
        if (event.isComposing) event.preventDefault()
        event.stopPropagation()
      }}
      onCloseAutoFocus={(event) => {
        event.preventDefault()
        trigger?.focus({ preventScroll: true })
      }}
    >
      <div class="reasoning-heading">
        <p>{t('chat.reasoningStrength')}: <strong>{effortLabel}</strong></p>
        <Button
          variant="secondary"
          size="xs"
          disabled={disabled || value === 'default'}
          onclick={() => choose('default')}>{t('chat.resetReasoning')}</Button
        >
      </div>
      {#if allowedEfforts.length === 0}
        <p class="reasoning-hint">{t('chat.reasoningUnconfigured')}</p>
        {#if onOpenSettings}<Button
            variant="outline"
            size="sm"
            onclick={() => {
              open = false
              onOpenSettings()
            }}>{t('chat.openSettings')}</Button
          >{/if}
      {:else if allowedEfforts.length > 1}
        <div class="reasoning-scale" aria-hidden="true">
          <span>{t('chat.reasoningFaster')}</span><span>{t('chat.reasoningSmarter')}</span>
        </div>
        <ReasoningSlider
          efforts={allowedEfforts}
          value={effectiveEffort}
          defaultIndex={effortIndex}
          {disabled}
          label={t('chat.reasoningEffort')}
          valueText={effortLabel}
          onchange={choose}
        />
      {:else}
        <Field.FieldSet {disabled}>
          <Field.FieldLegend class="sr-only">{t('chat.reasoningEffort')}</Field.FieldLegend>
          {#each ['default' as const, ...allowedEfforts] as effort (effort)}
            <label class="reasoning-option">
              <input
                type="radio"
                name={id}
                value={effort}
                checked={effectiveEffort === effort}
                onchange={() => choose(effort)}
              />
              {t(`chat.effort.${effort}`)}
            </label>
          {/each}
        </Field.FieldSet>
      {/if}
      {#if effectiveEffort === 'default'}<p class="reasoning-hint">
          {t('chat.reasoningDefaultHint')}
        </p>{/if}
    </Popover.Content>
  </Popover.Portal>
</Popover.Root>

<style>
  :global(.reasoning-popover) {
    box-sizing: border-box;
    z-index: 210;
    width: min(18rem, calc(100vw - 1rem));
    max-height: calc(100dvh - 1rem);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 0.75rem;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
    box-shadow: 0 4px 12px color-mix(in oklch, var(--color-foreground) 12%, transparent);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  .reasoning-heading,
  .reasoning-scale {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }
  .reasoning-heading p,
  .reasoning-hint {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .reasoning-scale,
  .reasoning-hint {
    color: var(--color-muted-foreground);
  }
  :global(.reasoning-popover input) {
    accent-color: var(--color-primary);
  }
  :global(.reasoning-popover input:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
  .reasoning-option {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
</style>
