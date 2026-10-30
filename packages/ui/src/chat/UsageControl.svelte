<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Popover } from 'bits-ui'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import { Button } from '#lib/components/ui/button/index.js'
  import type { KitI18n } from '../i18n'

  let {
    context = null,
    i18n,
    settings,
    details,
    open = $bindable(false),
  }: {
    context?: {
      tokens: number | null
      contextWindow: number | null
      modelName: string
      source: 'reported' | 'estimated' | 'unknown'
    } | null
    i18n: KitI18n
    settings?: Snippet | undefined
    details?: Snippet | undefined
    open?: boolean
  } = $props()
  const t = (key: string) => i18n.t(key)
  const percent = $derived(
    context?.tokens != null && context.contextWindow != null && context.contextWindow > 0
      ? Math.min(100, Math.max(0, (context.tokens / context.contextWindow) * 100))
      : null,
  )
  const format = (value: number | null | undefined) =>
    value == null ? t('chat.unknown') : value.toLocaleString(i18n.locale)
  let trigger = $state<HTMLButtonElement | null>(null)
</script>

<Popover.Root bind:open>
  <Popover.Trigger>
    {#snippet child({ props })}
      <Button
        {...props}
        bind:ref={trigger}
        variant="ghost"
        size="icon-sm"
        aria-label={t('chat.usageSettings')}
        title={t('chat.usageSettings')}
      >
        <Settings2Icon aria-hidden="true" />
      </Button>
    {/snippet}
  </Popover.Trigger>
  <Popover.Portal>
    <Popover.Content
      class="usage-popover"
      side="top"
      align="end"
      sideOffset={8}
      collisionPadding={12}
      aria-label={t('chat.usageSettings')}
      trapFocus={false}
      preventScroll={false}
      onEscapeKeydown={(event) => {
        if (event.isComposing) event.preventDefault()
        event.stopPropagation()
      }}
      onCloseAutoFocus={(event) => {
        event.preventDefault()
        trigger?.focus({ preventScroll: true })
      }}
    >
      <section class="context-summary" aria-label={t('chat.contextUsage')}>
        <h3>{t('chat.contextUsage')}</h3>
        {#if percent !== null}
          <meter min="0" max="100" value={percent} aria-label={t('chat.contextUsage')}></meter>
        {/if}
        <div class="context-values">
          <span
            >{format(context?.tokens)} / {format(context?.contextWindow)}{#if percent !== null}
              ({Math.round(percent)}%){/if}</span
          >
          {#if context?.modelName}<span class="context-model" title={context.modelName}>{context.modelName}</span>{/if}
        </div>
        <small
          >{t(
            context?.source === 'reported'
              ? 'chat.reported'
              : context?.source === 'estimated'
                ? 'chat.estimated'
                : 'chat.unknown',
          )}</small
        >
      </section>
      {#if settings}<section class="usage-settings" aria-label={t('chat.runSettings')}>
          <h3>{t('chat.runSettings')}</h3>
          {@render settings()}
        </section>{/if}
      {#if details}<details class="usage-more">
          <summary>{t('chat.usage')}</summary>
          <div class="usage-more-content">{@render details()}</div>
        </details>{/if}
    </Popover.Content>
  </Popover.Portal>
</Popover.Root>

<style>
  /* Hallmark · component: usage-settings · theme: inherited Aether
   * Pre-emit critique: Philosophy 5 · Hierarchy 5 · Execution 4 · Specificity 5 · Restraint 5 · Variety 4 */
  :global(.usage-popover) {
    box-sizing: border-box;
    z-index: 210;
    width: min(360px, calc(100vw - 24px));
    max-height: min(70dvh, var(--bits-popover-content-available-height));
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 16px;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  h3 {
    margin: 0;
    font: inherit;
    font-weight: 500;
  }
  .context-summary {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  .context-values {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    color: var(--color-muted-foreground);
    font-variant-numeric: tabular-nums;
  }
  .context-values > span:first-child {
    flex: none;
  }
  .context-model {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  small {
    color: var(--color-muted-foreground);
    font-size: 11px;
  }
  meter {
    appearance: none;
    width: 100%;
    height: 6px;
    border: 0;
    border-radius: 999px;
    background: var(--color-muted);
  }
  meter::-webkit-meter-bar {
    border: 0;
    border-radius: 999px;
    background: var(--color-muted);
  }
  meter::-webkit-meter-optimum-value {
    background: var(--color-primary);
    border-radius: 999px;
  }
  meter::-moz-meter-bar {
    background: var(--color-primary);
    border-radius: 999px;
  }
  .usage-settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
    border-top: 1px solid var(--color-border);
    padding-top: 12px;
  }
  .usage-more {
    border-top: 1px solid var(--color-border);
    padding-top: 12px;
  }
  summary {
    cursor: pointer;
    color: var(--color-muted-foreground);
  }
  summary:hover {
    color: var(--color-foreground);
  }
  summary:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
  .usage-more-content {
    padding-top: 12px;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  @media (max-width: 375px) {
    .context-values {
      align-items: flex-start;
      flex-direction: column;
      gap: 4px;
    }
    .context-model {
      max-width: 100%;
    }
  }
</style>
