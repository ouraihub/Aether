<script lang="ts">
  import type { ContextUsage, TokenUsage } from '../ports'
  import { getKitContext } from '../context'
  interface Props {
    context: ContextUsage | null
    usage: TokenUsage | null
    total: TokenUsage
  }
  let { context, usage, total }: Props = $props()
  const { t } = getKitContext().i18n
  const percent = $derived(
    context?.tokens != null && context.contextWindow != null && context.contextWindow > 0
      ? Math.min(100, Math.max(0, (context.tokens / context.contextWindow) * 100))
      : null,
  )
  const fields = ['inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens'] as const
</script>

<details class="text-muted-foreground text-kit-sm min-w-0" aria-label={t('chat.usage')}>
  <summary class="cursor-pointer"
    >{t('chat.contextUsage')}: {percent === null ? t('chat.unknown') : `${percent.toFixed(1)}%`}</summary
  >
  <div class="flex flex-col gap-2 py-2">
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
          >{/each}</tbody
      >
    </table>
    <p>{t('chat.usageHint')}</p>
  </div>
</details>
