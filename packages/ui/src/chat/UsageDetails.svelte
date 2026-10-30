<script lang="ts">
  import type { ContextUsage, Cost, TokenUsage } from '../ports'
  import { getKitContext } from '../context'
  import UsageControl from './UsageControl.svelte'
  interface Props {
    context: ContextUsage | null
    usage: TokenUsage | null
    total: TokenUsage
    /** Last turn's cost; null when its model has no pricing or a round lacked token counts. */
    cost: Cost | null
    /** Session total per currency. */
    totalCost: Cost[]
  }
  let { context, usage, total, cost, totalCost, modelName = '' }: Props & { modelName?: string } = $props()
  const { i18n } = getKitContext()
  const { t } = i18n
  const fields = ['inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens'] as const
  const money = ({ amount, currency }: Cost) => `${currency === 'CNY' ? '¥' : '$'}${amount.toFixed(4)}`
</script>

<UsageControl context={context ? { ...context, modelName } : null} {i18n}>
  {#snippet details()}
    <table>
      <thead
        ><tr
          ><th scope="col">{t('chat.usage')}</th><th scope="col">{t('chat.currentUsage')}</th><th scope="col"
            >{t('chat.totalUsage')}</th
          ></tr
        ></thead
      >
      <tbody
        >{#each fields as field (field)}<tr
            ><th scope="row">{t(`chat.usage.${field}`)}</th><td>{usage?.[field] ?? t('chat.unknown')}</td><td
              >{total[field] ?? t('chat.unknown')}</td
            ></tr
          >{/each}<tr
          ><th scope="row">{t('chat.cost')}</th><td>{cost ? money(cost) : t('chat.unknown')}</td><td
            >{totalCost.length ? totalCost.map(money).join(' + ') : t('chat.unknown')}</td
          ></tr
        ></tbody
      >
    </table>
    <p>{t('chat.usageHint')}</p>
  {/snippet}
</UsageControl>

<style>
  p {
    margin: 0;
    overflow-wrap: anywhere;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    padding: 0.25rem;
    text-align: left;
    overflow-wrap: anywhere;
  }
</style>
