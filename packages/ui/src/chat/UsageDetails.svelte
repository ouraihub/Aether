<script lang="ts">
  import type { ContextUsage, Cost, TokenUsage } from '../ports'
  import { getKitContext } from '../context'
  import ChartNoAxesColumnIcon from '@lucide/svelte/icons/chart-no-axes-column'
  let details = $state<HTMLDetailsElement | null>(null)
  interface Props {
    context: ContextUsage | null
    usage: TokenUsage | null
    total: TokenUsage
    /** Last turn's cost; null when its model has no pricing or a round lacked token counts. */
    cost: Cost | null
    /** Session total per currency. */
    totalCost: Cost[]
  }
  let { context, usage, total, cost, totalCost }: Props = $props()
  const { t } = getKitContext().i18n
  const percent = $derived(
    context?.tokens != null && context.contextWindow != null && context.contextWindow > 0
      ? Math.min(100, Math.max(0, (context.tokens / context.contextWindow) * 100))
      : null,
  )
  const fields = ['inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens'] as const
  const money = ({ amount, currency }: Cost) => `${currency === 'CNY' ? '¥' : '$'}${amount.toFixed(4)}`
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions (Esc closes this disclosure first) -->
<details
  class="usage-details"
  bind:this={details}
  aria-label={t('chat.usage')}
  onkeydown={(e) => {
    if (e.key === 'Escape' && details?.open) {
      details.open = false
      e.preventDefault()
      e.stopPropagation()
    }
  }}
>
  <summary aria-label={t('chat.usage')} title={t('chat.contextUsage')}>
    <ChartNoAxesColumnIcon aria-hidden="true" />
    <span>{percent === null ? t('chat.unknown') : `${percent.toFixed(1)}%`}</span>
  </summary>
  <div class="usage-content">
    <p>
      {t('chat.contextUsage')}: {context?.tokens ?? t('chat.unknown')}{#if context?.contextWindow != null}
        / {context.contextWindow}{/if} · {t(
        context?.source === 'reported'
          ? 'chat.reported'
          : context?.source === 'estimated'
            ? 'chat.estimated'
            : 'chat.unknown',
      )}
    </p>
    {#if percent !== null}<meter min="0" max="100" value={percent} aria-label={t('chat.contextUsage')}></meter>{/if}
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
  </div>
</details>

<style>
  .usage-details {
    position: relative;
    min-width: 0;
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  summary {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    cursor: pointer;
    padding: 0.25rem;
    border-radius: var(--kit-radius, var(--radius));
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary:focus-visible {
    outline: 2px solid var(--color-ring);
  }
  summary :global(svg) {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
  }
  .usage-content {
    position: absolute;
    bottom: calc(100% + 0.5rem);
    right: 0;
    width: min(20rem, calc(100cqw - 5rem));
    max-height: 45cqh;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
  }
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
  meter {
    width: 100%;
  }
</style>
