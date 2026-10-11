<script lang="ts">
  // One tool call collapsed to a line (docs/16 §4.2): calling, then its outcome and summary.
  import { getKitContext } from '../context'
  import type { ToolItem } from './run-session.svelte'

  let { tool }: { tool: ToolItem } = $props()
  const { t } = getKitContext().i18n
  const name = $derived(tool.server ? `${tool.name} · ${tool.server}` : tool.name)
</script>

<p class="text-muted-foreground truncate text-xs">
  {#if tool.outcome === null}
    {t('chat.tool.calling', { name })}
  {:else if tool.outcome === 'denied'}
    {t('chat.tool.denied', { name })}
  {:else if tool.outcome === 'unknown'}
    {t('chat.tool.unknown', { name })}
  {:else if tool.outcome === 'error'}
    {t('chat.tool.error', { name, summary: tool.summary })}
  {:else}
    {t('chat.tool.ok', { name, summary: tool.summary })}
  {/if}
</p>
