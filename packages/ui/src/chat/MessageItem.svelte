<script lang="ts">
  // One turn: user input, tool calls (with approval), assistant markdown (sanitized, set via
  // attachment), copy / insert, terminal state with retry.
  import { Button } from '#lib/components/ui/button/index.js'
  import { getKitContext } from '../context'
  import { renderMarkdown } from '../markdown'
  import ApprovalCard from './ApprovalCard.svelte'
  import type { ChatTurn, ToolItem } from './run-session.svelte'
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link'
  import { onDestroy } from 'svelte'
  import CopyIcon from '@lucide/svelte/icons/copy'
  import CheckIcon from '@lucide/svelte/icons/check'
  import FileInputIcon from '@lucide/svelte/icons/file-input'
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw'
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
  let copyTimer: ReturnType<typeof setTimeout> | undefined
  let alive = true
  onDestroy(() => {
    alive = false
    clearTimeout(copyTimer)
  })

  function insert() {
    try {
      onInsert?.(turn.text)
    } catch {
      notify.toast('error', t('errors.internal'))
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(turn.text)
    } catch {
      notify.toast('error', t('chat.copyFailed'))
      return
    }
    if (!alive) return
    copied = true
    clearTimeout(copyTimer)
    copyTimer = setTimeout(() => (copied = false), 1500)
  }
</script>

<li class="message-turn flex min-w-0 flex-col gap-2">
  <p class="user-message">{turn.input}</p>
  <div class="assistant-message">
    {#if turn.reasoningText}
      <details class="thinking text-muted-foreground text-kit-sm">
        <summary class="cursor-pointer">{t('chat.reasoning')}</summary>
        <p class="whitespace-pre-wrap">{turn.reasoningText}</p>
      </details>
    {/if}
    {#each turn.tools as tool (tool.callId)}
      {#if tool.pending === null}
        <ToolCallItem {tool} />
      {:else}
        <ApprovalCard {tool} onAnswer={(allow) => onApprove(tool, allow)} />
      {/if}
    {/each}
    <div
      class="markdown"
      {@attach (node) => {
        node.innerHTML = html
      }}
    ></div>
    {#if turn.text !== '' || (onRetry && turn.status !== 'running')}
      <div class="message-actions">
        {#if turn.text !== ''}
          <Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.copy')}
            aria-label={t('chat.copy')}
            onclick={() => void copy()}
          >
            {#if copied}<CheckIcon />{:else}<CopyIcon />{/if}
          </Button>
          {#if onInsert}
            <Button
              variant="ghost"
              size="icon-xs"
              title={t('chat.insert')}
              aria-label={t('chat.insert')}
              disabled={turn.status === 'running'}
              onclick={insert}
            >
              <FileInputIcon />
            </Button>
          {/if}
        {/if}
        {#if onRetry && turn.status !== 'running'}
          {@const label = t(turn.status === 'failed' ? 'chat.retry' : 'chat.regenerate')}
          <Button variant="ghost" size="icon-xs" title={label} aria-label={label} onclick={onRetry}
            ><RotateCcwIcon /></Button
          >
        {/if}
        <span class="text-muted-foreground text-kit-sm" role="status">{copied ? t('chat.copied') : ''}</span>
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
      <p class="text-muted-foreground text-kit-sm" role="status">{t('chat.stopped')}</p>
    {:else if turn.status === 'failed'}
      <div class="flex items-center gap-2">
        <p class="text-destructive text-kit-sm" role="alert">
          {turn.error === 'interrupted' ? t('chat.interrupted') : t(`errors.${turn.error ?? 'internal'}`)}
        </p>
      </div>
    {:else if turn.exhausted}
      <p class="text-muted-foreground text-kit-sm">{t('chat.exhausted')}</p>
    {/if}
  </div>
</li>

<style>
  .message-turn,
  .user-message,
  .assistant-message,
  .markdown {
    box-sizing: border-box;
    min-width: 0;
    max-width: 100%;
    overflow-wrap: anywhere;
  }
  .user-message {
    align-self: flex-end;
    margin: 0;
    padding: 0.625rem 0.75rem;
    border-radius: var(--kit-radius);
    background: var(--color-primary);
    color: var(--color-primary-foreground);
    font-size: var(--kit-text-base);
    white-space: pre-wrap;
  }
  .assistant-message {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 0.75rem;
    border-radius: var(--kit-radius);
    background: var(--color-muted);
    color: var(--color-foreground);
  }
  .thinking {
    min-width: 0;
  }
  .message-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.25rem;
    border-top: 1px solid var(--color-border);
    padding-top: 0.5rem;
  }
  .markdown {
    font-size: var(--kit-text-base);
    line-height: 1.6;
  }
  .markdown :global(p),
  .markdown :global(ul),
  .markdown :global(ol),
  .markdown :global(blockquote),
  .markdown :global(pre),
  .markdown :global(table) {
    margin: 0.75em 0;
  }
  .markdown :global(h1),
  .markdown :global(h2),
  .markdown :global(h3),
  .markdown :global(h4),
  .markdown :global(h5),
  .markdown :global(h6) {
    margin: 1em 0 0.5em;
    font-weight: 600;
    line-height: 1.35;
  }
  .markdown :global(h1) {
    font-size: 1.4em;
  }
  .markdown :global(h2) {
    font-size: 1.25em;
  }
  .markdown :global(h3) {
    font-size: 1.1em;
  }
  .markdown :global(ul),
  .markdown :global(ol) {
    padding-inline-start: 1.5em;
  }
  .markdown :global(ul) {
    list-style: disc;
  }
  .markdown :global(ol) {
    list-style: decimal;
  }
  .markdown :global(li) {
    margin: 0.25em 0;
  }
  .markdown :global(li::marker) {
    color: var(--color-muted-foreground);
  }
  .markdown :global(blockquote) {
    border-inline-start: 3px solid var(--color-border);
    padding-inline-start: 0.75em;
    color: var(--color-muted-foreground);
  }
  .markdown :global(a) {
    color: var(--color-primary);
    text-decoration: underline;
    overflow-wrap: anywhere;
  }
  .markdown :global(code) {
    border-radius: var(--kit-radius-sm);
    background: var(--color-background);
    padding: 0.1em 0.3em;
    font-size: 0.95em;
  }
  .markdown :global(pre) {
    max-width: 100%;
    overflow-x: auto;
    padding: 0.75em;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius-sm);
    background: var(--color-background);
    white-space: pre;
    overflow-wrap: normal;
  }
  .markdown :global(pre code) {
    padding: 0;
    background: none;
  }
  .markdown :global(table) {
    display: block;
    max-width: 100%;
    overflow-x: auto;
    border-collapse: collapse;
  }
  .markdown :global(th),
  .markdown :global(td) {
    padding: 0.4em 0.6em;
    border: 1px solid var(--color-border);
  }
  .markdown :global(th) {
    font-weight: 600;
  }
  .markdown :global(img),
  .markdown :global(video) {
    max-width: 100%;
    height: auto;
  }
  .markdown :global(:first-child) {
    margin-top: 0;
  }
  .markdown :global(:last-child) {
    margin-bottom: 0;
  }
</style>
