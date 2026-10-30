<script lang="ts">
  import type { Snippet } from 'svelte'
  import { renderMarkdown } from '../markdown'
  import { Button } from '#lib/components/ui/button/index.js'

  let {
    role,
    text,
    label,
    status,
    reasoning,
    reasoningLabel = 'Reasoning',
    process,
    attachments,
    children,
    actions,
    latest = false,
    selected = false,
    showMoreLabel = 'Show more',
    showLessLabel = 'Show less',
  }: {
    role: 'user' | 'assistant'
    text: string
    label?: string | undefined
    status?: string | undefined
    reasoning?: string | undefined
    reasoningLabel?: string | undefined
    process?: Snippet | undefined
    attachments?: Snippet | undefined
    children?: Snippet | undefined
    actions?: Snippet | undefined
    latest?: boolean | undefined
    selected?: boolean | undefined
    showMoreLabel?: string | undefined
    showLessLabel?: string | undefined
  } = $props()
  const html = $derived(role === 'assistant' ? renderMarkdown(text) : '')
  const preview = $derived.by(() => {
    if (role !== 'user') return null
    let lines = 0
    let end = 0
    for (const line of text.split('\n')) {
      if (line.trim() && ++lines > 5) return text.slice(0, end).trimEnd()
      end += line.length + 1
    }
    return null
  })
  let expanded = $state(false)
</script>

<div
  class="chat-message"
  data-status={status}
  class:user={role === 'user'}
  class:assistant={role === 'assistant'}
  class:latest
  class:selected
>
  {#if label}<div class="reply-source">{label}</div>{/if}
  {#if attachments}{@render attachments()}{/if}
  {#if reasoning}<details class="thinking">
      <summary>{reasoningLabel}</summary>
      <p>{reasoning}</p>
    </details>{/if}
  {#if process}{@render process()}{/if}
  {#if role === 'user'}
    <div class="user-bubble">
      <p class="user-message">{preview !== null && !expanded ? preview : text}</p>
      {#if preview !== null}<Button
          type="button"
          variant="ghost"
          size="sm"
          aria-expanded={expanded}
          onclick={() => {
            expanded = !expanded
          }}>{expanded ? showLessLabel : showMoreLabel}</Button
        >{/if}
    </div>
  {:else}
    <div
      class="markdown"
      {@attach (node) => {
        node.innerHTML = html
      }}
    ></div>
  {/if}
  {#if children}{@render children()}{/if}
  {#if actions}<div class="message-actions">{@render actions()}</div>{/if}
</div>

<style>
  /* Hallmark · component: open assistant message · theme: host semantic tokens */
  .chat-message {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    min-width: 0;
    max-width: 100%;
    overflow-wrap: anywhere;
    color: var(--color-foreground);
  }
  .assistant {
    padding-block: 0.75rem;
  }
  .selected {
    outline: 1px solid var(--color-border);
    outline-offset: 0.5rem;
    border-radius: var(--kit-radius, var(--radius));
  }
  .user {
    align-items: flex-end;
  }
  .user-bubble {
    max-width: min(90%, 42rem);
    border-radius: var(--kit-radius, var(--radius));
    padding: 0.625rem 0.875rem;
    background: var(--color-muted);
  }
  .user-message {
    margin: 0;
    font-size: var(--kit-text-base);
    white-space: pre-wrap;
  }
  .reply-source,
  .thinking {
    color: var(--color-muted-foreground);
    font-size: var(--kit-text-sm);
  }
  .thinking summary {
    cursor: pointer;
  }
  .thinking summary:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
  .thinking p {
    white-space: pre-wrap;
    margin-block: 0.5rem;
  }
  .message-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.25rem;
    min-height: 2rem;
  }
  .message-actions :global(button) {
    min-width: 2rem;
    min-height: 2rem;
  }
  @media (hover: hover) {
    .assistant:not(.latest) .message-actions {
      opacity: 0;
    }
    .assistant:hover .message-actions,
    .assistant:focus-within .message-actions {
      opacity: 1;
    }
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
