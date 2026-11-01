<script lang="ts">
  import { getKitContext } from '../context'
  import MinusIcon from '@lucide/svelte/icons/minus'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import type { Model, PricingTier } from '../ports'
  import type { ModelField } from './draft'

  let {
    edit,
    errorFor,
  }: { edit: Model | null; errorFor: (field: ModelField) => { message: string }[] } = $props()
  const { t } = getKitContext().i18n
  const PRICES: (keyof PricingTier)[] = ['input', 'output', 'cacheRead', 'cacheWrite']
  const CURRENCIES = [
    ['USD', '$'],
    ['CNY', '¥'],
  ] as const
  const CHIP = 'aria-pressed:border-primary aria-pressed:bg-primary/10 aria-pressed:text-primary'
  const count = (value: number) => (Number.isNaN(value) ? null : value)
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
</script>

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
              oninput={(event) =>
                setPrice(index, 'minInputTokens', event.currentTarget.valueAsNumber)}
            /></label
          >
          {#each PRICES as key (key)}
            <label class="flex flex-col gap-1 text-kit-xs"
              >{t(`providers.price.${key}`)}<Input
                type="number"
                min="0"
                step="any"
                placeholder={key === 'cacheRead' || key === 'cacheWrite'
                  ? t('providers.price_same_as_input')
                  : ''}
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
          onclick={() =>
            current.tiers.push(tier((current.tiers.at(-1)?.minInputTokens ?? 0) + 200000))}
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
