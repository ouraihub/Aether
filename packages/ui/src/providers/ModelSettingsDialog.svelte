<script lang="ts">
  // Per-model parameters (Cherry's EditModelDrawer). Edits a copy; nothing changes until Save.
  import { untrack } from 'svelte'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { getKitContext } from '../context'
  import type { Model, Protocol, ReasoningDialect, ReasoningEffort } from '../ports'
  import { REASONING_DIALECTS, REASONING_EFFORTS, emptyReasoning } from './reasoning'

  let {
    open = $bindable(false),
    model,
    protocol,
    onsave,
  }: { open: boolean; model: Model | null; protocol: Protocol; onsave: (model: Model) => void } = $props()

  const { t } = getKitContext().i18n
  let edit = $state<Model | null>(null)
  $effect.pre(() => {
    if (open) edit = untrack(() => (model ? $state.snapshot(model) : null))
  })
  const dialects = $derived(
    REASONING_DIALECTS.filter((dialect) =>
      protocol === 'anthropic-messages' ? dialect !== 'openai' : dialect === 'openai',
    ),
  )
  const count = (value: number) => (Number.isNaN(value) ? null : value)

  function setDialect(value: string) {
    if (edit) edit.reasoning = value === 'unknown' ? null : emptyReasoning(value as ReasoningDialect)
  }
  function toggleEffort(effort: ReasoningEffort) {
    const config = edit?.reasoning
    if (!edit || !config) return
    const efforts = config.efforts.includes(effort)
      ? config.efforts.filter((item) => item !== effort)
      : [...config.efforts, effort]
    edit.reasoning = { ...config, efforts }
  }
  function setBudget(value: number) {
    if (!edit?.reasoning) return
    const rest = { ...edit.reasoning }
    delete rest.budgetTokens
    edit.reasoning = Number.isNaN(value) ? rest : { ...rest, budgetTokens: value }
  }
  function save() {
    if (edit) onsave($state.snapshot(edit))
    open = false
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85dvh] min-w-0 flex-col sm:max-w-md">
    <Dialog.Header class="min-w-0 text-left">
      <Dialog.Title class="truncate pr-6 font-mono" title={edit?.id}>{edit?.id}</Dialog.Title>
      <Dialog.Description>{t('providers.model_settings')}</Dialog.Description>
    </Dialog.Header>
    {#if edit}
      <Field.Group class="min-h-0 overflow-y-auto">
        {#if edit.modelType === 'chat'}
          <Field.Field orientation="horizontal">
            <Switch id="kit-model-vision" bind:checked={edit.canVision} />
            <Field.Label for="kit-model-vision">{t('providers.vision')}</Field.Label>
          </Field.Field>
          {#if protocol !== 'openai-chat'}
            <Field.Field orientation="horizontal">
              <Switch id="kit-model-web" bind:checked={edit.nativeWebSearch} />
              <Field.Label for="kit-model-web">{t('providers.nativeWebSearch')}</Field.Label>
            </Field.Field>
          {/if}
          <Field.Field>
            <Field.Label for="kit-model-dialect">{t('providers.reasoning_dialect')}</Field.Label>
            <Select.Root type="single" value={edit.reasoning?.dialect ?? 'unknown'} onValueChange={setDialect}>
              <Select.Trigger id="kit-model-dialect" class="w-full"
                >{edit.reasoning?.dialect ?? t('chat.unknown')}</Select.Trigger
              >
              <Select.Content>
                <Select.Group>
                  <Select.Item value="unknown" label={t('chat.unknown')} />
                  {#each dialects as dialect (dialect)}<Select.Item value={dialect} label={dialect} />{/each}
                </Select.Group>
              </Select.Content>
            </Select.Root>
          </Field.Field>
          {#if edit.reasoning}
            <Field.Set>
              <Field.Legend>{t('providers.reasoning_efforts')}</Field.Legend>
              <div class="flex flex-wrap gap-3">
                {#each REASONING_EFFORTS as effort (effort)}
                  <label class="flex items-center gap-1 text-kit-sm"
                    ><input
                      type="checkbox"
                      checked={edit.reasoning.efforts.includes(effort)}
                      onchange={() => toggleEffort(effort)}
                    />{t(`chat.effort.${effort}`)}</label
                  >
                {/each}
              </div>
            </Field.Set>
            {#if edit.reasoning.dialect === 'anthropic-budget'}
              <Field.Field>
                <Field.Label for="kit-model-budget">{t('providers.reasoning_budget')}</Field.Label>
                <Input
                  id="kit-model-budget"
                  type="number"
                  min="1024"
                  step="1"
                  value={edit.reasoning.budgetTokens ?? ''}
                  onchange={(event) => setBudget(event.currentTarget.valueAsNumber)}
                />
              </Field.Field>
            {/if}
          {/if}
        {/if}
        <Field.Field>
          <Field.Label for="kit-model-context">{t('providers.context_window')}</Field.Label>
          <Input
            id="kit-model-context"
            type="number"
            min="1"
            step="1"
            value={edit.contextWindow ?? ''}
            onchange={(event) => edit && (edit.contextWindow = count(event.currentTarget.valueAsNumber))}
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="kit-model-output">{t('providers.max_output')}</Field.Label>
          <Input
            id="kit-model-output"
            type="number"
            min="1"
            step="1"
            value={edit.maxOutput ?? ''}
            onchange={(event) => edit && (edit.maxOutput = count(event.currentTarget.valueAsNumber))}
          />
        </Field.Field>
      </Field.Group>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{t('common.cancel')}</Button>
      <Button onclick={save}>{t('providers.save')}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
