<script lang="ts">
  // Host-owned model setting (docs/16 §4.1): the value is stored by the app; a deleted model shows as invalid.
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { ModelFilter, ModelOption, ModelRef } from '../ports'

  let {
    value,
    filter = {},
    onchange,
    placeholder,
  }: {
    value: ModelRef | null
    filter?: ModelFilter
    onchange: (value: ModelRef) => void
    placeholder?: string
  } = $props()

  const { ports, i18n } = getKitContext()
  const { t } = i18n
  let options = $state<ModelOption[] | null>(null)

  const key = (r: ModelRef) => `${r.endpointId}\n${r.modelId}`
  $effect(() => {
    void ports.providers?.modelOptions(filter).then((list) => (options = list))
  })
  const current = $derived(value && options?.find((o) => key(o) === key(value)))
  const invalid = $derived(value !== null && options !== null && !current)
</script>

{#if ports.providers}
  <Select.Root
    type="single"
    value={value ? key(value) : ''}
    onValueChange={(next) => {
      const option = options?.find((o) => key(o) === next)
      if (option) onchange({ endpointId: option.endpointId, modelId: option.modelId })
    }}
  >
    <Select.Trigger class="w-full" aria-invalid={invalid}>
      {current?.label ?? (invalid ? t('providers.model_invalid') : (placeholder ?? t('providers.model_placeholder')))}
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
