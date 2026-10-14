<script lang="ts">
  // Host-owned model setting (docs/16 §4.1): the value is stored by the app; a deleted model shows as invalid.
  import * as Select from '#lib/components/ui/select/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import { getKitContext } from '../context'
  import type { ModelFilter, ModelOption, ModelRef } from '../ports'

  let {
    value,
    filter = {},
    onchange,
    placeholder,
    disabled = false,
    options: suppliedOptions,
  }: {
    value: ModelRef | null
    filter?: ModelFilter
    onchange: (value: ModelRef, option: ModelOption | null) => void
    placeholder?: string
    disabled?: boolean
    /** Reuses capabilities already loaded by a containing panel; null means loading. */
    options?: ModelOption[] | null
  } = $props()

  const { ports, i18n } = getKitContext()
  const { t } = i18n
  let loadedOptions = $state<ModelOption[] | null>(null)
  let failed = $state(false)
  let reload = $state(0)
  const options = $derived(suppliedOptions === undefined ? loadedOptions : suppliedOptions)

  const key = (r: ModelRef) => `${r.endpointId}\n${r.modelId}`
  $effect(() => {
    if (suppliedOptions !== undefined) return
    void reload
    loadedOptions = null
    failed = false
    let alive = true
    void ports.providers
      ?.modelOptions(filter)
      .then((list) => {
        if (alive) loadedOptions = list
      })
      .catch(() => {
        if (alive) failed = true
      })
    return () => {
      alive = false
    }
  })
  const current = $derived(value && options?.find((o) => key(o) === key(value)))
  const invalid = $derived(!failed && value !== null && options !== null && !current)
</script>

{#if ports.providers}
  {#if failed}
    <div class="model-error" role="alert">
      <span>{t('chat.modelsFailed')}</span>
      <Button variant="outline" size="sm" {disabled} onclick={() => reload++}>{t('chat.retry')}</Button>
    </div>
  {:else}
    <Select.Root
      type="single"
      value={value ? key(value) : ''}
      onValueChange={(next) => {
        const option = options?.find((o) => key(o) === next) ?? null
        if (option) onchange({ endpointId: option.endpointId, modelId: option.modelId }, option)
      }}
    >
      <Select.Trigger
        class="w-full"
        disabled={disabled || options === null || options.length === 0}
        aria-invalid={invalid}
        aria-label={t('providers.model_placeholder')}
        title={current?.label}
      >
        <span class="model-label"
          >{options === null
            ? t('chat.modelsLoading')
            : (current?.label ??
              (invalid ? t('providers.model_invalid') : (placeholder ?? t('providers.model_placeholder'))))}</span
        >
      </Select.Trigger>
      <Select.Content>
        <Select.Group>
          {#each options ?? [] as option (key(option))}
            <Select.Item value={key(option)} label={option.label} />
          {/each}
        </Select.Group>
      </Select.Content>
    </Select.Root>
  {/if}
{/if}

<style>
  .model-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .model-error {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    color: var(--color-muted-foreground);
  }
</style>
