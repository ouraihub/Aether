<script lang="ts">
  import { tick, untrack, type Snippet } from 'svelte'
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down'
  import { Button } from '#lib/components/ui/button/index.js'

  let {
    label,
    messagesLabel = label,
    header,
    children,
    composer,
    navigation,
    followKey,
    scrollKey,
    latestLabel = 'Back to latest',
    class: className = '',
  }: {
    label: string
    messagesLabel?: string | undefined
    header?: Snippet | undefined
    children: Snippet
    composer: Snippet
    navigation?: Snippet | undefined
    followKey?: string | number | undefined
    scrollKey?: string | undefined
    latestLabel?: string | undefined
    class?: string | undefined
  } = $props()
  let scroller = $state<HTMLDivElement | null>(null)
  let follow = $state(true)
  let lastScrollTop = 0
  let viewportHeight = $state(0)
  let contentHeight = $state(0)
  function toBottom() {
    follow = true
    if (scroller) {
      scroller.scrollTop = scroller.scrollHeight
      lastScrollTop = scroller.scrollTop
    }
  }
  function onscroll() {
    if (!scroller) return
    follow =
      scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <= 48 && scroller.scrollTop >= lastScrollTop
    lastScrollTop = scroller.scrollTop
  }
  $effect.pre(() => {
    void followKey
    void viewportHeight
    void contentHeight
    if (untrack(() => follow))
      void tick().then(() => {
        if (follow) toBottom()
      })
  })
  $effect(() => {
    void scrollKey
    follow = true
    void tick().then(toBottom)
  })
</script>

<section class="chat-surface {className}" aria-label={label}>
  {#if navigation}<aside class="chat-navigation">{@render navigation()}</aside>{/if}
  <div class="chat-main">
    {#if header}<div class="surface-header">{@render header()}</div>{/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard-accessible scroll region) -->
    <div
      class="messages"
      bind:this={scroller}
      bind:clientHeight={viewportHeight}
      {onscroll}
      role="region"
      aria-label={messagesLabel}
      tabindex="0"
    >
      <div class="message-column" bind:clientHeight={contentHeight}>{@render children()}</div>
    </div>
    {#if !follow}<div class="bottom-control">
        <Button type="button" variant="secondary" size="sm" onclick={toBottom}
          ><ArrowDownIcon data-icon="inline-start" />{latestLabel}</Button
        >
      </div>{/if}
    <div class="surface-composer">{@render composer()}</div>
  </div>
</section>

<style>
  /* Hallmark · component: conversation surface · theme: host semantic tokens */
  .chat-surface {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    height: 100%;
    container-type: inline-size;
    color: var(--color-foreground);
  }
  .chat-main {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    position: relative;
    container-type: size;
  }
  .chat-navigation {
    flex: 0 0 240px;
    min-height: 0;
    overflow-y: auto;
    border-inline-end: 1px solid var(--color-border);
  }
  .surface-header {
    flex: none;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--color-border);
  }
  .messages {
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow-y: auto;
    overflow-x: clip;
    overscroll-behavior: contain;
    overflow-anchor: none;
  }
  .messages:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }
  .message-column,
  .surface-composer {
    box-sizing: border-box;
    width: 100%;
    max-width: 848px;
    margin-inline: auto;
    padding: 1.5rem;
  }
  .surface-composer {
    flex: none;
    max-height: 70cqh;
    overflow-y: auto;
    padding-top: 0.5rem;
    padding-bottom: 1rem;
  }
  .bottom-control {
    display: flex;
    flex: none;
    justify-content: center;
    padding-block: 0.25rem;
  }
  @container (max-width: 899px) {
    .chat-navigation {
      display: none;
    }
  }
  @container (max-width: 480px) {
    .message-column,
    .surface-composer {
      padding-inline: 0.75rem;
    }
    .surface-header {
      padding-inline: 0.75rem;
    }
  }
</style>
