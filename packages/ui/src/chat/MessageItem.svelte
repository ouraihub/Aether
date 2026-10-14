<script lang="ts">
  // One turn: user input, tool calls (with approval), assistant markdown (sanitized, set via
  // attachment), copy / insert, terminal state with retry.
  import { Button } from '#lib/components/ui/button/index.js'
  import { getKitContext } from '../context'
  import { renderMarkdown } from '../markdown'
  import ApprovalCard from './ApprovalCard.svelte'
  import type { ChatTurn, ToolItem } from './run-session.svelte'
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link'
  import ToolCallItem from './ToolCallItem.svelte'

  let {
    turn,
    onApprove,
    onInsert,
    onRetry,
  }: {
    turn: ChatTurn
    onApprove: (tool: ToolItem, allow: boolean) => void
    onInsert?: ((markdown: string) => void) | undefined
    /** Only for the last turn while nothing runs. */
    onRetry?: (() => void) | undefined
  } = $props()
  const { i18n, notify } = getKitContext()
  const { t } = i18n
  const html = $derived(renderMarkdown(turn.text))
  function safeUrl(value: string): string | null {
    try {
      const protocol = new URL(value).protocol
      return protocol === 'http:' || protocol === 'https:' ? value : null
    } catch {
      return null
    }
  }
  let copied = $state(false)

  async function copy() {
    try {
      await navigator.clipboard.writeText(turn.text)
    } catch {
      notify.toast('error', t('chat.copyFailed'))
      return
    }
    copied = true
    setTimeout(() => (copied = false), 1500)
  }
</script>

<li class="flex flex-col gap-2">
  {#if turn.reasoningText}
    <details class="text-muted-foreground text-kit-sm">
      <summary class="cursor-pointer">{t('chat.reasoning')}</summary>
      <p class="whitespace-pre-wrap">{turn.reasoningText}</p>
    </details>
  {/if}

  <p class="bg-muted rounded-kit text-kit-base self-end px-3 py-2 whitespace-pre-wrap">{turn.input}</p>
  {#each turn.tools as tool (tool.callId)}
    {#if tool.pending === null}
      <ToolCallItem {tool} />
    {:else}
      <ApprovalCard {tool} onAnswer={(allow) => onApprove(tool, allow)} />
    {/if}
  {/each}
  <div
    class="text-kit-base max-w-none"
    aria-live={turn.status === 'running' ? 'polite' : 'off'}
    {@attach (node) => {
      node.innerHTML = html
    }}
  ></div>
  {#if turn.text !== ''}
    <div class="flex gap-1">
      <Button variant="ghost" size="xs" onclick={() => void copy()}>{copied ? t('chat.copied') : t('chat.copy')}</Button
      >
      {#if onInsert}
        <Button variant="ghost" size="xs" disabled={turn.status === 'running'} onclick={() => onInsert(turn.text)}
          >{t('chat.insert')}</Button
        >
      {/if}
    </div>
  {/if}
  {#if turn.search.status === 'complete'}
    <section class="flex flex-col gap-1 text-kit-sm" aria-label={t('chat.sources')}>
      <p class="text-muted-foreground">{t('chat.sources')}</p>
      {#each turn.search.hits as hit (hit.url)}
        {@const url = safeUrl(hit.url)}
        {#if url}<a
            class="text-primary flex min-w-0 items-center gap-1 underline"
            href={url}
            target="_blank"
            rel="noopener noreferrer"
            ><ExternalLinkIcon data-icon="inline-start" /><span class="truncate">{hit.title || url}</span></a
          >{/if}
      {/each}
    </section>
  {/if}
  {#if turn.status === 'cancelled'}
    <p class="text-muted-foreground text-kit-sm">{t('chat.stopped')}</p>
  {:else if turn.status === 'failed'}
    <div class="flex items-center gap-2">
      <p class="text-destructive text-kit-sm" role="alert">
        {turn.error === 'interrupted' ? t('chat.interrupted') : t(`errors.${turn.error ?? 'internal'}`)}
      </p>
      {#if onRetry}
        <Button variant="outline" size="xs" onclick={onRetry}>{t('chat.retry')}</Button>
      {/if}
    </div>
  {:else if turn.exhausted}
    <p class="text-muted-foreground text-kit-sm">{t('chat.exhausted')}</p>
  {/if}
</li>
