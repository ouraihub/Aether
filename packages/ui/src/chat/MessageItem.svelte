<script lang="ts">
  // One turn: user input, tool calls (with approval), assistant markdown (sanitized, set via
  // attachment), terminal state.
  import { getKitContext } from '../context'
  import { renderMarkdown } from '../markdown'
  import ApprovalCard from './ApprovalCard.svelte'
  import type { ChatTurn, ToolItem } from './run-session.svelte'
  import ToolCallItem from './ToolCallItem.svelte'

  let { turn, onApprove }: { turn: ChatTurn; onApprove: (tool: ToolItem, allow: boolean) => void } = $props()
  const { t } = getKitContext().i18n
  const html = $derived(renderMarkdown(turn.text))
</script>

<li class="flex flex-col gap-2">
  <p class="bg-muted self-end rounded-lg px-3 py-2 text-sm whitespace-pre-wrap">{turn.input}</p>
  {#each turn.tools as tool (tool.callId)}
    {#if tool.pending === null}
      <ToolCallItem {tool} />
    {:else}
      <ApprovalCard {tool} onAnswer={(allow) => onApprove(tool, allow)} />
    {/if}
  {/each}
  <div
    class="prose prose-sm max-w-none text-sm"
    aria-live={turn.status === 'running' ? 'polite' : 'off'}
    {@attach (node) => {
      node.innerHTML = html
    }}
  ></div>
  {#if turn.status === 'cancelled'}
    <p class="text-muted-foreground text-xs">{t('chat.stopped')}</p>
  {:else if turn.status === 'failed'}
    <p class="text-destructive text-xs" role="alert">
      {turn.error === 'interrupted' ? t('chat.interrupted') : t(`errors.${turn.error ?? 'internal'}`)}
    </p>
  {:else if turn.exhausted}
    <p class="text-muted-foreground text-xs">{t('chat.exhausted')}</p>
  {/if}
</li>
