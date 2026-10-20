<script lang="ts">
  // Per-model settings (Cherry's EditModelDrawer, docs/41 §3–§4). Edits a copy; nothing changes until Save.
  import { untrack } from 'svelte'
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right'
  import MinusIcon from '@lucide/svelte/icons/minus'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import CopyInput from '../CopyInput.svelte'
  import { getKitContext } from '../context'
  import type { Model, ModelType, PricingTier, Protocol, ReasoningDialect, ReasoningEffort } from '../ports'
  import { cn } from '../utils'
  import { idPrefix, purposeOf, validateModel, withPurpose, withType, type ModelField, type Purpose } from './draft'
  import { REASONING_DIALECTS, REASONING_EFFORTS, emptyReasoning } from './reasoning'

  let {
    open = $bindable(false),
    model,
    protocol,
    usedBy = [],
    onsave,
  }: {
    open: boolean
    model: Model | null
    protocol: Protocol
    /** Localized role names bound to this model (contract §6). */
    usedBy?: string[]
    onsave: (model: Model) => void
  } = $props()

  const MAIN_TYPES = ['chat', 'image', 'embedding', 'rerank'] as const
  const OTHER_TYPES: ModelType[] = ['video', 'tts', 'stt', 'unknown']
  // Cherry's chip values mix decimal and binary sizes, so labels are explicit.
  const CONTEXT_PRESETS: [string, number][] = [
    ['128K', 128000],
    ['200K', 200000],
    ['256K', 262144],
    ['400K', 400000],
    ['512K', 524288],
    ['1M', 1000000],
  ]
  const OUTPUT_PRESETS: [string, number][] = [
    ['16K', 16384],
    ['32K', 32768],
    ['64K', 65536],
    ['128K', 131072],
  ]
  const PRICES = ['input', 'output', 'cacheRead', 'cacheWrite'] as const
  const PURPOSES: Purpose[] = ['chat', 'image', 'edit']
  const CURRENCIES = [
    ['USD', '$'],
    ['CNY', '¥'],
  ] as const
  const CHIP = 'aria-pressed:border-primary aria-pressed:bg-primary/10 aria-pressed:text-primary'

  const { t } = getKitContext().i18n
  let edit = $state<Model | null>(null)
  let more = $state(true)
  $effect.pre(() => {
    if (!open) return
    edit = untrack(() => (model ? $state.snapshot(model) : null))
    more = true
  })
  const dialects = $derived(
    REASONING_DIALECTS.filter((dialect) =>
      protocol === 'anthropic-messages' ? dialect !== 'openai' : dialect === 'openai',
    ),
  )
  const errors = $derived(edit ? validateModel(edit, protocol) : [])
  const purpose = $derived(edit && purposeOf(edit))
  const count = (value: number) => (Number.isNaN(value) ? null : value)

  function errorFor(field: ModelField) {
    return errors
      .filter((item) => item.field === field)
      .map((item) => ({ message: t(`providers.error.${field}.${item.reason}`) }))
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
  /** Tri-state: not probed → supported → unsupported → not probed. */
  function cycleTools() {
    if (edit) edit.supportsTools = edit.supportsTools === null ? true : edit.supportsTools ? false : null
  }
  const tier = (minInputTokens: number): PricingTier => ({
    minInputTokens,
    input: 0,
    output: 0,
    cacheRead: null,
    cacheWrite: null,
  })
  function setPrice(index: number, key: keyof PricingTier, value: number) {
    const target = edit?.pricing?.tiers[index]
    if (!target) return
    // Required prices stay NaN when cleared so validation flags them; cache prices fall back to the input price.
    if (key === 'cacheRead' || key === 'cacheWrite') target[key] = count(value)
    else target[key] = value
  }
  function removeTier(index: number) {
    const pricing = edit?.pricing
    if (!edit || !pricing) return
    pricing.tiers.splice(index, 1)
    if (pricing.tiers[0]) pricing.tiers[0].minInputTokens = 0
    else edit.pricing = null
  }
  function save() {
    if (!edit || errors.length > 0) return
    const name = edit.name?.trim() || null
    const groupName = edit.groupName?.trim() || null
    onsave({ ...$state.snapshot(edit), name, groupName })
    open = false
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85dvh] min-w-0 flex-col sm:max-w-lg">
    <Dialog.Header class="min-w-0 text-left">
      <Dialog.Title>{t('providers.edit_model')}</Dialog.Title>
    </Dialog.Header>
    {#if edit}
      <Field.Group class="min-h-0 overflow-y-auto pr-1">
        <Field.Field>
          <Field.Label for="kit-model-id">{t('providers.model_id')}</Field.Label>
          <CopyInput id="kit-model-id" class="font-mono" readonly value={edit.id} />
        </Field.Field>
        <Field.Field>
          <Field.Label for="kit-model-name">{t('providers.model_name')}</Field.Label>
          <Input
            id="kit-model-name"
            placeholder={edit.id}
            autocomplete="off"
            aria-invalid={errorFor('name').length > 0}
            bind:value={() => edit?.name ?? '', (value) => edit && (edit.name = value)}
          />
          <Field.Error errors={errorFor('name')} />
        </Field.Field>
        <Field.Field>
          <Field.Label for="kit-model-group">{t('providers.group_name')}</Field.Label>
          <Input
            id="kit-model-group"
            placeholder={idPrefix(edit.id)}
            autocomplete="off"
            aria-invalid={errorFor('groupName').length > 0}
            bind:value={() => edit?.groupName ?? '', (value) => edit && (edit.groupName = value)}
          />
          <Field.Error errors={errorFor('groupName')} />
        </Field.Field>
        {#if purpose}
          <Field.Set>
            <Field.Legend variant="label">{t('providers.purpose')}</Field.Legend>
            <div class="flex flex-wrap gap-4">
              {#each PURPOSES as item (item)}
                <label class="flex items-center gap-1.5 text-kit-sm"
                  ><input
                    type="radio"
                    name="kit-model-purpose"
                    checked={purpose === item}
                    onchange={() => edit && (edit = withPurpose(edit, item))}
                  />{t(`providers.purpose.${item}`)}</label
                >
              {/each}
            </div>
          </Field.Set>
        {/if}
        <Button
          variant="ghost"
          size="sm"
          class="justify-start self-start px-1"
          aria-expanded={more}
          onclick={() => (more = !more)}
          ><ChevronRightIcon class={cn('transition-transform', more && 'rotate-90')} />{t(
            'providers.more_settings',
          )}</Button
        >
        {#if more}
          <Field.Set>
            <Field.Legend variant="label">{t('providers.model_type')}</Field.Legend>
            <div class="flex flex-wrap gap-1.5">
              {#each MAIN_TYPES as type (type)}
                <Button
                  variant="outline"
                  size="sm"
                  class={CHIP}
                  aria-pressed={edit.modelType === type}
                  onclick={() => edit && (edit = withType(edit, type))}>{t(`providers.type.${type}`)}</Button
                >
              {/each}
              <Select.Root
                type="single"
                value={OTHER_TYPES.includes(edit.modelType) ? edit.modelType : ''}
                onValueChange={(value) => edit && value && (edit = withType(edit, value as ModelType))}
              >
                <Select.Trigger
                  size="sm"
                  class={cn(OTHER_TYPES.includes(edit.modelType) && 'border-primary bg-primary/10 text-primary')}
                  aria-label={t('providers.type_other')}
                  >{OTHER_TYPES.includes(edit.modelType)
                    ? t(`providers.type.${edit.modelType}`)
                    : t('providers.type_other')}</Select.Trigger
                >
                <Select.Content>
                  <Select.Group>
                    {#each OTHER_TYPES as type (type)}<Select.Item
                        value={type}
                        label={t(`providers.type.${type}`)}
                      />{/each}
                  </Select.Group>
                </Select.Content>
              </Select.Root>
            </div>
            {#if usedBy.length > 0 && edit.modelType !== model?.modelType}
              <Field.Description class="text-warning text-kit-xs" role="status"
                >{t('providers.type_in_use', { roles: usedBy.join(t('providers.list_separator')) })}</Field.Description
              >
            {/if}
          </Field.Set>
          {#if edit.modelType === 'chat'}
            <Field.Set>
              <Field.Legend variant="label">{t('providers.capabilities')}</Field.Legend>
              <div class="flex flex-wrap gap-1.5">
                <Button
                  variant="outline"
                  size="sm"
                  class={CHIP}
                  aria-pressed={edit.reasoning !== null}
                  onclick={() =>
                    edit && (edit.reasoning = edit.reasoning ? null : emptyReasoning(dialects[0] ?? 'openai'))}
                  >{t('providers.cap_reasoning')}</Button
                >
                <Button
                  variant="outline"
                  size="sm"
                  class={cn(CHIP, edit.supportsTools === false && 'text-muted-foreground line-through')}
                  aria-pressed={edit.supportsTools === true}
                  title={t(`providers.tools_state.${edit.supportsTools ?? 'unknown'}`)}
                  onclick={cycleTools}
                  >{t('providers.cap_tools')}{#if edit.supportsTools === null}<span
                      class="text-muted-foreground text-kit-xs">{t('providers.tools_state.unknown')}</span
                    >{/if}</Button
                >
                {#if protocol !== 'openai-chat'}
                  <Button
                    variant="outline"
                    size="sm"
                    class={CHIP}
                    aria-pressed={edit.nativeWebSearch}
                    title={t('providers.nativeWebSearch')}
                    onclick={() => edit && (edit.nativeWebSearch = !edit.nativeWebSearch)}
                    >{t('providers.cap_web')}</Button
                  >
                {/if}
              </div>
            </Field.Set>
            {#if edit.reasoning}
              {@render reasoningControls(edit.reasoning.dialect)}
            {/if}
            <Field.Set>
              <Field.Legend variant="label">{t('providers.input_modalities')}</Field.Legend>
              <div class="flex flex-wrap gap-1.5">
                <Button
                  variant="outline"
                  size="sm"
                  class={CHIP}
                  aria-pressed={edit.canVision}
                  onclick={() => edit && (edit.canVision = !edit.canVision)}>{t('providers.cap_vision')}</Button
                >
              </div>
            </Field.Set>
          {/if}
          {@render tokens('contextWindow', CONTEXT_PRESETS, 32000)}
          {@render tokens('maxOutput', OUTPUT_PRESETS, 4096)}
          {#if edit.modelType === 'chat'}
            {@render pricing()}
          {/if}
        {/if}
      </Field.Group>
    {/if}
    <Dialog.Footer class="items-center">
      <p class="text-muted-foreground mr-auto text-kit-xs">{t('providers.applies_next')}</p>
      <Button variant="outline" onclick={() => (open = false)}>{t('common.cancel')}</Button>
      <Button disabled={errors.length > 0} onclick={save}>{t('providers.save')}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

{#snippet reasoningControls(dialect: ReasoningDialect)}
  <Field.Field>
    <Field.Label for="kit-model-dialect">{t('providers.reasoning_dialect')}</Field.Label>
    <Select.Root
      type="single"
      value={dialect}
      onValueChange={(value) => edit && (edit.reasoning = emptyReasoning(value as ReasoningDialect))}
    >
      <Select.Trigger id="kit-model-dialect" class="w-full">{dialect}</Select.Trigger>
      <Select.Content>
        <Select.Group>
          {#each dialects as item (item)}<Select.Item value={item} label={item} />{/each}
        </Select.Group>
      </Select.Content>
    </Select.Root>
  </Field.Field>
  <Field.Set>
    <Field.Legend variant="label">{t('providers.reasoning_efforts')}</Field.Legend>
    <div class="flex flex-wrap gap-3">
      {#each REASONING_EFFORTS as effort (effort)}
        <label class="flex items-center gap-1 text-kit-sm"
          ><input
            type="checkbox"
            checked={edit?.reasoning?.efforts.includes(effort)}
            onchange={() => toggleEffort(effort)}
          />{t(`chat.effort.${effort}`)}</label
        >
      {/each}
    </div>
    {#if dialect !== 'anthropic-budget'}<Field.Error errors={errorFor('reasoning')} />{/if}
  </Field.Set>
  {#if dialect === 'anthropic-budget'}
    <Field.Field>
      <Field.Label for="kit-model-budget">{t('providers.reasoning_budget')}</Field.Label>
      <Input
        id="kit-model-budget"
        type="number"
        min="1024"
        step="1"
        aria-invalid={errorFor('reasoning').length > 0}
        value={edit?.reasoning?.budgetTokens ?? ''}
        oninput={(event) => setBudget(event.currentTarget.valueAsNumber)}
      />
      <Field.Error errors={errorFor('reasoning')} />
    </Field.Field>
  {/if}
{/snippet}

{#snippet tokens(field: 'contextWindow' | 'maxOutput', presets: [string, number][], fallback: number)}
  {@const value = edit?.[field] ?? null}
  <Field.Field>
    <Field.Label for="kit-model-{field}"
      >{t(`providers.${field === 'contextWindow' ? 'context_window' : 'max_output'}`)}</Field.Label
    >
    <div class="flex flex-wrap gap-1.5">
      {#each presets as [label, preset] (preset)}
        <Button
          variant="outline"
          size="sm"
          class={CHIP}
          aria-pressed={value === preset}
          onclick={() => edit && (edit[field] = preset)}>{label}</Button
        >
      {/each}
    </div>
    <Input
      id="kit-model-{field}"
      type="number"
      min="1"
      step="1"
      placeholder={String(fallback)}
      aria-invalid={errorFor(field).length > 0}
      value={value ?? ''}
      oninput={(event) => edit && (edit[field] = count(event.currentTarget.valueAsNumber))}
    />
    <Field.Error errors={errorFor(field)} />
  </Field.Field>
{/snippet}

{#snippet pricing()}
  <Field.Set>
    <Field.Legend variant="label">{t('providers.pricing')}</Field.Legend>
    {#if edit?.pricing}
      {@const current = edit.pricing}
      <div class="flex flex-wrap gap-1.5" role="group" aria-label={t('providers.currency')}>
        {#each CURRENCIES as [currency, symbol] (currency)}
          <Button
            variant="outline"
            size="sm"
            class={CHIP}
            aria-pressed={current.currency === currency}
            aria-label={t(`providers.currency.${currency}`)}
            onclick={() => (current.currency = currency)}>{symbol}</Button
          >
        {/each}
      </div>
      <p class="text-muted-foreground text-kit-xs">{t('providers.pricing_unit')}</p>
      {#each current.tiers as item, index (index)}
        <fieldset class="flex flex-col gap-2 rounded-kit border p-2">
          <legend class="flex w-full items-center gap-2 px-1 text-kit-sm"
            >{t('providers.tier', { n: index + 1 })}{#if current.tiers.length > 1}<Button
                variant="ghost"
                size="icon-xs"
                class="ml-auto"
                aria-label={t('providers.tier_remove', { n: index + 1 })}
                title={t('providers.tier_remove', { n: index + 1 })}
                onclick={() => removeTier(index)}><MinusIcon /></Button
              >{/if}</legend
          >
          <div class="grid grid-cols-2 gap-2 sm:grid-cols-3">
            <label class="flex flex-col gap-1 text-kit-xs"
              >{t('providers.tier_start')}<Input
                type="number"
                min="0"
                step="1"
                disabled={index === 0}
                value={item.minInputTokens}
                oninput={(event) => setPrice(index, 'minInputTokens', event.currentTarget.valueAsNumber)}
              /></label
            >
            {#each PRICES as key (key)}
              <label class="flex flex-col gap-1 text-kit-xs"
                >{t(`providers.price.${key}`)}<Input
                  type="number"
                  min="0"
                  step="any"
                  placeholder={key === 'cacheRead' || key === 'cacheWrite' ? t('providers.price_same_as_input') : ''}
                  value={item[key] ?? ''}
                  oninput={(event) => setPrice(index, key, event.currentTarget.valueAsNumber)}
                /></label
              >
            {/each}
          </div>
        </fieldset>
      {/each}
      <Field.Error errors={errorFor('pricing')} />
      <div class="flex flex-wrap gap-1.5">
        {#if current.tiers.length < 8}<Button
            variant="outline"
            size="sm"
            onclick={() => current.tiers.push(tier((current.tiers.at(-1)?.minInputTokens ?? 0) + 200000))}
            ><PlusIcon />{t('providers.tier_add')}</Button
          >{/if}
        <Button variant="ghost" size="sm" onclick={() => edit && (edit.pricing = null)}
          >{t('providers.pricing_clear')}</Button
        >
      </div>
    {:else}
      <Button
        variant="outline"
        size="sm"
        class="self-start"
        onclick={() => edit && (edit.pricing = { currency: 'USD', tiers: [tier(0)] })}
        ><PlusIcon />{t('providers.pricing_add')}</Button
      >
    {/if}
  </Field.Set>
{/snippet}
