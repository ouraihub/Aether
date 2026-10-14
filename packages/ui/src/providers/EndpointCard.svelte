<script lang="ts">
  // One endpoint: collapsed summary or the edit form (docs/16 §4.1). The parent keeps one card expanded.
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { Protocol, Model, ReasoningDialect } from '../ports'
  import { probeUrl, type EndpointDraft } from './draft'
  import SecretInput from './SecretInput.svelte'
  import { REASONING_DIALECTS, REASONING_EFFORTS, emptyReasoning } from './reasoning'

  let {
    endpoint = $bindable(),
    index,
    expanded,
    fetching,
    ontoggle,
    onfetch,
    onremove,
  }: {
    endpoint: EndpointDraft
    index: number
    expanded: boolean
    fetching: boolean
    ontoggle: () => void
    onfetch: () => void
    onremove: () => void
  } = $props()

  const PROTOCOLS: Protocol[] = ['openai-chat', 'openai-responses', 'anthropic-messages']
  const { t } = getKitContext().i18n
  const prefix = $derived(`kit-endpoint-${index}`)
  let selectedModelId = $state('')
  const selectedModel = $derived(endpoint.models.find((model) => model.id === selectedModelId) ?? endpoint.models[0])
  const selectedReasoning = $derived(selectedModel?.reasoning ?? null)
  const dialects = $derived(
    REASONING_DIALECTS.filter((dialect) =>
      endpoint.protocol === 'anthropic-messages' ? dialect !== 'openai' : dialect === 'openai',
    ),
  )
  function updateModel(update: (model: Model) => void) {
    if (!selectedModel) return
    update(selectedModel)
    endpoint.models = [...endpoint.models]
  }
  function toggleEffort(effort: (typeof REASONING_EFFORTS)[number]) {
    updateModel((model) => {
      if (!model.reasoning) return
      const config = model.reasoning
      const efforts = config.efforts.includes(effort)
        ? config.efforts.filter((item) => item !== effort)
        : [...config.efforts, effort]
      model.reasoning = { ...config, efforts }
    })
  }
</script>

<div class="flex flex-col gap-3 rounded-kit border p-3">
  <button
    type="button"
    class="flex items-center gap-2 text-left text-kit-base"
    aria-expanded={expanded}
    onclick={ontoggle}
  >
    {#if expanded}<ChevronDownIcon />{:else}<ChevronRightIcon />{/if}
    <span class="font-medium">{endpoint.slug || '—'}</span>
    <span class="text-muted-foreground flex-1 truncate">{endpoint.baseUrl}</span>
    <span class="text-muted-foreground">{t('providers.model_count', { count: endpoint.models.length })}</span>
  </button>
  {#if expanded}
    <Field.Group>
      <div class="flex gap-3">
        <Field.Field class="w-32">
          <Field.Label for="{prefix}-slug">{t('providers.slug')}</Field.Label>
          <Input id="{prefix}-slug" bind:value={endpoint.slug} />
        </Field.Field>
        <Field.Field class="flex-1">
          <Field.Label for="{prefix}-protocol">{t('providers.protocol')}</Field.Label>
          <Select.Root type="single" bind:value={endpoint.protocol}>
            <Select.Trigger id="{prefix}-protocol" class="w-full">{endpoint.protocol}</Select.Trigger>
            <Select.Content>
              <Select.Group>
                {#each PROTOCOLS as protocol (protocol)}
                  <Select.Item value={protocol} label={protocol} />
                {/each}
              </Select.Group>
            </Select.Content>
          </Select.Root>
        </Field.Field>
      </div>
      <Field.Field>
        <Field.Label for="{prefix}-url">{t('providers.base_url')}</Field.Label>
        <Input id="{prefix}-url" placeholder="https://api.example.com/v1" bind:value={endpoint.baseUrl} />
        {#if endpoint.baseUrl.trim()}
          <Field.Description class="font-mono">→ GET {probeUrl(endpoint.baseUrl)}</Field.Description>
        {/if}
      </Field.Field>
      <Field.Field>
        <Field.Label for="{prefix}-key">{t('providers.api_key')}</Field.Label>
        <SecretInput id="{prefix}-key" saved={endpoint.saved} bind:value={endpoint.key} />
      </Field.Field>
      <Field.Field>
        <Field.Label>{t('providers.models')}</Field.Label>
        <Field.Description>
          {endpoint.models.length === 0 ? t('providers.no_models') : endpoint.models.map((m) => m.id).join(', ')}
        </Field.Description>
      </Field.Field>
      {#if selectedModel}
        <Field.Field>
          <Field.Label for="{prefix}-model">{t('providers.model_settings')}</Field.Label>
          <Select.Root type="single" value={selectedModel.id} onValueChange={(value) => (selectedModelId = value)}>
            <Select.Trigger id="{prefix}-model" class="w-full">{selectedModel.id}</Select.Trigger>
            <Select.Content
              ><Select.Group
                >{#each endpoint.models as model (model.id)}<Select.Item
                    value={model.id}
                    label={model.id}
                  />{/each}</Select.Group
              ></Select.Content
            >
          </Select.Root>
        </Field.Field>
        {#if selectedModel.modelType === 'chat'}
          {#if endpoint.protocol !== 'openai-chat'}
            <Field.Field
              ><label class="flex items-center gap-2"
                ><input
                  type="checkbox"
                  checked={selectedModel.nativeWebSearch}
                  onchange={(event) => updateModel((model) => (model.nativeWebSearch = event.currentTarget.checked))}
                />{t('providers.nativeWebSearch')}</label
              ></Field.Field
            >
          {/if}
          <Field.Field>
            <Field.Label for="{prefix}-dialect">{t('providers.reasoning_dialect')}</Field.Label>
            <Select.Root
              type="single"
              value={selectedReasoning?.dialect ?? 'unknown'}
              onValueChange={(value) =>
                updateModel(
                  (model) => (model.reasoning = value === 'unknown' ? null : emptyReasoning(value as ReasoningDialect)),
                )}
            >
              <Select.Trigger id="{prefix}-dialect" class="w-full"
                >{selectedReasoning?.dialect ?? t('chat.unknown')}</Select.Trigger
              >
              <Select.Content
                ><Select.Group>
                  <Select.Item value="unknown" label={t('chat.unknown')} />
                  {#each dialects as dialect (dialect)}<Select.Item value={dialect} label={dialect} />{/each}
                </Select.Group></Select.Content
              >
            </Select.Root>
          </Field.Field>
          {#if selectedReasoning}
            <Field.Set>
              <Field.Legend>{t('providers.reasoning_efforts')}</Field.Legend>
              <div class="flex flex-wrap gap-3">
                {#each REASONING_EFFORTS as effort (effort)}
                  <label class="flex items-center gap-1"
                    ><input
                      type="checkbox"
                      checked={selectedReasoning.efforts.includes(effort)}
                      onchange={() => toggleEffort(effort)}
                    />{t(`chat.effort.${effort}`)}</label
                  >
                {/each}
              </div>
            </Field.Set>
            {#if selectedReasoning.dialect === 'anthropic-budget'}
              <Field.Field>
                <Field.Label for="{prefix}-budget">{t('providers.reasoning_budget')}</Field.Label>
                <Input
                  id="{prefix}-budget"
                  type="number"
                  min="1024"
                  step="1"
                  value={selectedReasoning.budgetTokens ?? ''}
                  onchange={(event) =>
                    updateModel((model) => {
                      if (model.reasoning) {
                        const next = event.currentTarget.valueAsNumber
                        if (Number.isNaN(next)) {
                          model.reasoning = { ...model.reasoning }
                          delete model.reasoning.budgetTokens
                        } else model.reasoning = { ...model.reasoning, budgetTokens: next }
                      }
                    })}
                />
              </Field.Field>
            {/if}
          {/if}
        {/if}
        <Field.Field>
          <Field.Label for="{prefix}-context">{t('providers.context_window')}</Field.Label>
          <Input
            id="{prefix}-context"
            type="number"
            min="1"
            step="1"
            value={selectedModel.contextWindow ?? ''}
            onchange={(event) =>
              updateModel(
                (model) =>
                  (model.contextWindow = Number.isNaN(event.currentTarget.valueAsNumber)
                    ? null
                    : event.currentTarget.valueAsNumber),
              )}
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="{prefix}-output">{t('providers.max_output')}</Field.Label>
          <Input
            id="{prefix}-output"
            type="number"
            min="1"
            step="1"
            value={selectedModel.maxOutput ?? ''}
            onchange={(event) =>
              updateModel(
                (model) =>
                  (model.maxOutput = Number.isNaN(event.currentTarget.valueAsNumber)
                    ? null
                    : event.currentTarget.valueAsNumber),
              )}
          />
        </Field.Field>
      {/if}
    </Field.Group>
    <div class="flex gap-2">
      <Button size="sm" disabled={fetching || !endpoint.baseUrl.trim()} onclick={onfetch}>
        {t('providers.fetch_models')}
      </Button>
      <Button size="sm" variant="ghost" onclick={onremove}>{t('providers.remove_endpoint')}</Button>
    </div>
  {/if}
</div>
