<script lang="ts">
  import type { ContextUsage, Cost, TokenUsage } from '../ports'
  import { getKitContext } from '../context'
  import ContextUsageStatus from './ContextUsageStatus.svelte'
  import type { SpendingTurn } from './context-status'
  interface Props {
    context: ContextUsage | null
    /** Selected model's window when the usage report has none. */
    contextWindow?: number | null | undefined
    /** Every turn's usage and cost; the inline summary prices only fully priced sessions. */
    turns: readonly SpendingTurn[]
    usage: TokenUsage | null
    total: TokenUsage
    /** Last turn's cost; null when its model has no pricing or a round lacked token counts. */
    cost: Cost | null
    /** Session total per currency. */
    totalCost: Cost[]
    open?: boolean
  }
  let {
    context,
    contextWindow = null,
    turns,
    usage,
    total,
    cost,
    totalCost,
    open = $bindable(false),
    modelName = '',
  }: Props & { modelName?: string } = $props()
  const { i18n } = getKitContext()
  const { t } = i18n
  const fields = [
    'inputTokens',
    'outputTokens',
    'cacheReadTokens',
    'cacheWriteTokens',
    'reasoningTokens',
  ] as const
  const money = ({ amount, currency }: Cost) =>
    `${currency === 'CNY' ? '¥' : '$'}${amount.toFixed(4)}`
</script>

<ContextUsageStatus
  bind:open
  {context}
  contextWindow={contextWindow ?? null}
  {turns}
  {modelName}
  {i18n}
>
  {#snippet details()}
    <table>
      <thead
        ><tr
          ><th scope="col">{t('chat.usage')}</th><th scope="col">{t('chat.currentUsage')}</th><th
            scope="col">{t('chat.totalUsage')}</th
          ></tr
        ></thead
      >
      <tbody
        >{#each fields as field (field)}<tr
            ><th scope="row">{t(`chat.usage.${field}`)}</th><td
              >{usage?.[field] ?? t('chat.unknown')}</td
            ><td>{total[field] ?? t('chat.unknown')}</td></tr
          >{/each}<tr
          ><th scope="row">{t('chat.cost')}</th><td>{cost ? money(cost) : t('chat.unknown')}</td><td
            >{totalCost.length ? totalCost.map(money).join(' + ') : t('chat.unknown')}</td
          ></tr
        ></tbody
      >
    </table>
    <p>{t('chat.usageHint')}</p>
  {/snippet}
</ContextUsageStatus>

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
