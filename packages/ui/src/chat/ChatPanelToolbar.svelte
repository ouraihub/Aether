<script lang="ts">
  import { tick } from 'svelte'
  import { Popover } from 'bits-ui'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import GlobeIcon from '@lucide/svelte/icons/globe'
  import PaperclipIcon from '@lucide/svelte/icons/paperclip'
  import MessageSquarePlusIcon from '@lucide/svelte/icons/message-square-plus'
  import ImageIcon from '@lucide/svelte/icons/image'
  import CheckIcon from '@lucide/svelte/icons/check'
  import GripVerticalIcon from '@lucide/svelte/icons/grip-vertical'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw'
  import XIcon from '@lucide/svelte/icons/x'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import type { KitI18n } from '../i18n'
  import type { ModelOption } from '../ports'
  let {
    i18n,
    running,
    historyBusy,
    vision,
    paintingModel,
    capability,
    purpose,
    panelWidth,
    toolbar,
    hasSearch,
    onToolbarChange,
    newConversation,
    attach,
    webSearch = $bindable(false),
    generateImage = $bindable(false),
  }: {
    i18n: KitI18n
    running: boolean
    historyBusy: boolean
    vision: boolean
    paintingModel: boolean
    capability: ModelOption | null
    purpose: string
    hasSearch: boolean
    panelWidth: number
    toolbar?: string[] | undefined
    onToolbarChange?: ((ids: string[]) => void) | undefined
    newConversation: () => void
    attach: (files: FileList | null) => Promise<void>
    webSearch?: boolean
    generateImage?: boolean
  } = $props()
  const t = $derived(i18n.t)
  const reasoningId = $props.id()
  const IMAGE_MIMES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif']
  let fileInput = $state<HTMLInputElement | null>(null)
  let toolsOpen = $state(false)
  let customizing = $state(false)
  const DEFAULT_TOOLBAR = ['new-conversation', 'web-search']
  // svelte-ignore state_referenced_locally
  let pinned = $state<string[]>(toolbar ?? DEFAULT_TOOLBAR)
  const visiblePinned = $derived(
    pinned
      .filter((id) => tools.includes(id))
      .slice(0, panelWidth > 0 && panelWidth < 480 ? 1 : pinned.length),
  )
  /** Tools usable here; unavailable pinned ids stay stored but hidden (docs/38 §3.2). */
  const tools = $derived(
    ['new-conversation', 'web-search', 'attachment', 'generate-image'].filter(
      (id) =>
        (id !== 'web-search' || (hasSearch && purpose === 'chat')) &&
        (id !== 'generate-image' || purpose === 'chat'),
    ),
  )
  const toolOrder = $derived([
    ...pinned.filter((id) => tools.includes(id)),
    ...tools.filter((id) => !pinned.includes(id)),
  ])
  function setPinned(next: string[]) {
    const all = [...next, ...pinned.filter((id) => !tools.includes(id))]
    if (all.join() === pinned.join()) return
    pinned = all
    onToolbarChange?.(all)
  }
  function moveTool(id: string, to: number) {
    const order = toolOrder.filter((item) => item !== id)
    order.splice(Math.max(0, Math.min(to, order.length)), 0, id)
    setPinned(order.filter((item) => pinned.includes(item)))
  }
  // Window listeners: keyed reordering moves the row node, which would drop pointer capture.
  function dragTool(event: PointerEvent, id: string) {
    const list = (event.currentTarget as HTMLElement).closest('ul')
    if (!list) return
    event.preventDefault()
    const move = (e: PointerEvent) => {
      const over = [...list.children].findIndex((row) => {
        const box = row.getBoundingClientRect()
        return e.clientY >= box.top && e.clientY < box.bottom
      })
      if (over >= 0 && toolOrder[over] !== id) moveTool(id, over)
    }
    const stop = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', stop)
      window.removeEventListener('pointercancel', stop)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', stop)
    window.addEventListener('pointercancel', stop)
  }
  const TOOL_LABEL: Record<string, string> = {
    'new-conversation': 'chat.newConversation',
    'web-search': 'chat.network',
    attachment: 'chat.uploadAttachment',
    'generate-image': 'chat.generateImage',
  }
</script>

{#snippet toolIcon(id: string)}
  {#if id === 'new-conversation'}<MessageSquarePlusIcon />{:else if id === 'web-search'}<GlobeIcon
      >{#if !webSearch}<path d="m2 2 20 20" />{/if}</GlobeIcon
    >{:else if id === 'attachment'}<PaperclipIcon />{:else}<ImageIcon />{/if}
{/snippet}
{#each visiblePinned as id (id)}
  {#if id === 'new-conversation'}<Button
      type="button"
      variant="ghost"
      size="icon-sm"
      disabled={running || historyBusy}
      aria-label={t('chat.newConversation')}
      title={t('chat.newConversation')}
      onclick={newConversation}>{@render toolIcon(id)}</Button
    >{:else if id === 'web-search'}<Button
      type="button"
      variant="ghost"
      class="search-toggle"
      size="icon-sm"
      aria-label={t('chat.network')}
      aria-pressed={webSearch}
      disabled={running}
      title={`${t(webSearch ? 'chat.networkOn' : 'chat.networkOff')} · ${t('chat.networkPrivacy')}`}
      onclick={() => (webSearch = !webSearch)}>{@render toolIcon(id)}</Button
    >{:else if id === 'attachment'}<Button
      type="button"
      variant="ghost"
      size="icon-sm"
      aria-label={t('chat.uploadAttachment')}
      disabled={running || !vision}
      title={vision ? t('chat.uploadAttachment') : t('chat.visionUnsupported')}
      onclick={() => fileInput?.click()}>{@render toolIcon(id)}</Button
    >{:else}<Button
      type="button"
      variant="ghost"
      class="search-toggle"
      size="icon-sm"
      aria-label={t('chat.generateImage')}
      aria-pressed={generateImage}
      disabled={running || !paintingModel || capability?.supportsTools === false}
      title={!paintingModel
        ? t('chat.paintingUnconfigured')
        : capability?.supportsTools === false
          ? t('chat.toolsUnsupported')
          : t('chat.generateImage')}
      onclick={() => (generateImage = !generateImage)}>{@render toolIcon(id)}</Button
    >{/if}
{/each}
<input
  bind:this={fileInput}
  type="file"
  accept={IMAGE_MIMES.join(',')}
  multiple
  hidden
  onchange={(e) => {
    const input = e.currentTarget
    void attach(input.files).finally(() => (input.value = ''))
  }}
/>
<Popover.Root bind:open={toolsOpen} onOpenChange={(open) => !open && (customizing = false)}>
  <Popover.Trigger disabled={running}>
    {#snippet child({ props })}<Button
        {...props}
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label={t('chat.tools')}
        title={t('chat.tools')}><PlusIcon /></Button
      >{/snippet}
  </Popover.Trigger>
  <Popover.Content class="tools-menu" side="top" align="start" sideOffset={6} collisionPadding={8}>
    {#if customizing}
      <p class="tools-title" id="{reasoningId}-toolbar">{t('chat.toolbarCustomize')}</p>
      <ul class="tools-order" aria-labelledby="{reasoningId}-toolbar">
        {#each toolOrder as id, index (id)}
          <li class="tools-row">
            <button
              type="button"
              class="tools-grip"
              aria-label={t('chat.toolbarDrag', { name: t(TOOL_LABEL[id] ?? id) })}
              onpointerdown={(e) => dragTool(e, id)}
              onkeydown={(e) => {
                if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
                e.preventDefault()
                const button = e.currentTarget
                moveTool(id, index + (e.key === 'ArrowUp' ? -1 : 1))
                void tick().then(() => button.focus())
              }}><GripVerticalIcon /></button
            >
            {@render toolIcon(id)}<span>{t(TOOL_LABEL[id] ?? id)}</span>
            <Switch
              size="sm"
              aria-label={t(TOOL_LABEL[id] ?? id)}
              checked={pinned.includes(id)}
              onCheckedChange={(on) =>
                setPinned(toolOrder.filter((item) => (item === id ? on : pinned.includes(item))))}
            />
          </li>
        {/each}
      </ul>
      <button
        type="button"
        class="tools-item tools-footer"
        disabled={pinned.join() === DEFAULT_TOOLBAR.join()}
        onclick={() => {
          pinned = [...DEFAULT_TOOLBAR]
          onToolbarChange?.(pinned)
        }}><RotateCcwIcon /><span>{t('chat.toolbarRestore')}</span></button
      >
    {:else}
      {#each toolOrder.filter((id) => !visiblePinned.includes(id)) as id (id)}
        {#if id === 'new-conversation'}<button
            type="button"
            class="tools-item"
            disabled={historyBusy}
            onclick={() => {
              toolsOpen = false
              newConversation()
            }}>{@render toolIcon(id)}<span>{t('chat.newConversation')}</span></button
          >{:else if id === 'web-search'}<button
            type="button"
            class="tools-item"
            aria-pressed={webSearch}
            onclick={() => {
              toolsOpen = false
              webSearch = !webSearch
            }}
            >{@render toolIcon(id)}<span>{t('chat.network')}</span>{#if webSearch}<CheckIcon
              />{/if}</button
          >{:else if id === 'attachment'}<button
            type="button"
            class="tools-item"
            disabled={!vision}
            title={vision ? undefined : t('chat.visionUnsupported')}
            onclick={() => {
              toolsOpen = false
              fileInput?.click()
            }}
            >{@render toolIcon(id)}<span>{t('chat.uploadAttachment')}</span>{#if !vision}<small
                >{t('chat.visionUnsupported')}</small
              >{/if}</button
          >{:else}<button
            type="button"
            class="tools-item"
            aria-pressed={generateImage}
            disabled={!paintingModel || capability?.supportsTools === false}
            title={!paintingModel
              ? t('chat.paintingUnconfigured')
              : capability?.supportsTools === false
                ? t('chat.toolsUnsupported')
                : undefined}
            onclick={() => {
              toolsOpen = false
              generateImage = !generateImage
            }}
            >{@render toolIcon(id)}<span>{t('chat.generateImage')}</span>{#if !paintingModel}<small
                >{t('chat.paintingUnconfigured')}</small
              >{:else if generateImage}<CheckIcon />{/if}</button
          >{/if}
      {/each}
      <button
        type="button"
        class="tools-item tools-footer"
        aria-label={t('chat.toolbarCustomize')}
        onclick={() => (customizing = true)}
        ><span>{t('chat.quickPanel')}</span><Settings2Icon /></button
      >
    {/if}
  </Popover.Content>
</Popover.Root>
{#if generateImage && paintingModel && purpose === 'chat' && !pinned.includes('generate-image')}<Button
    type="button"
    variant="secondary"
    size="sm"
    class="tool-chip"
    aria-label={t('chat.generateImageOff')}
    title={t('chat.generateImageOff')}
    onclick={() => (generateImage = false)}><ImageIcon />{t('chat.generateImage')}<XIcon /></Button
  >{/if}

<style>
  :global(button.search-toggle) {
    color: var(--color-muted-foreground);
  }
  :global(button.search-toggle[aria-pressed='true']) {
    color: var(--color-primary-foreground);
    background: var(--color-primary);
    border-color: var(--color-primary);
  }
  :global(button.search-toggle[aria-pressed='true']:hover:not(:disabled)) {
    background: color-mix(in oklch, var(--color-primary) 85%, var(--color-foreground));
  }
  :global(.tools-menu) {
    z-index: 50;
    box-sizing: border-box;
    width: min(16rem, calc(100vw - 1rem));
    display: flex;
    flex-direction: column;
    padding: 0.25rem;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
    box-shadow: 0 4px 12px color-mix(in oklch, var(--color-foreground) 12%, transparent);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  :global(.tools-item) {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-height: 2.25rem;
    padding: 0.25rem 0.625rem;
    border: 0;
    border-radius: calc(var(--kit-radius, var(--radius)) - 2px);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  :global(.tools-item:hover:not(:disabled)),
  :global(.tools-item:focus-visible) {
    background: var(--color-accent);
    color: var(--color-accent-foreground);
    outline: none;
  }
  :global(.tools-item:disabled) {
    cursor: not-allowed;
    opacity: 0.55;
  }
  :global(.tools-item svg) {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
  }
  :global(.tools-item small) {
    margin-left: auto;
    color: var(--color-muted-foreground);
    font-size: 0.75rem;
  }
  :global(.tools-item > svg:last-child:not(:first-child)) {
    margin-left: auto;
  }
  :global(.tools-footer) {
    margin-top: 0.25rem;
    border-top: 1px solid var(--color-border);
    border-radius: 0 0 calc(var(--kit-radius, var(--radius)) - 2px)
      calc(var(--kit-radius, var(--radius)) - 2px);
    color: var(--color-muted-foreground);
  }
  :global(.tools-title) {
    margin: 0;
    padding: 0.375rem 0.5rem;
    font-size: 0.875rem;
    font-weight: 500;
  }
  :global(.tools-order) {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  :global(.tools-row) {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-height: 2rem;
    padding: 0 0.375rem;
    border-radius: calc(var(--kit-radius, var(--radius)) - 2px);
    font-size: 0.875rem;
  }
  :global(.tools-row:hover) {
    background: var(--color-accent);
  }
  :global(.tools-row > svg) {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
    color: var(--color-muted-foreground);
  }
  :global(.tools-row > span) {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  :global(.tools-grip) {
    display: inline-flex;
    padding: 0.125rem;
    border: 0;
    border-radius: 0.25rem;
    background: none;
    color: var(--color-muted-foreground);
    cursor: grab;
    touch-action: none;
  }
  :global(.tools-grip:focus-visible) {
    outline: 2px solid var(--color-ring);
  }
  :global(.tools-grip svg) {
    width: 0.875rem;
    height: 0.875rem;
  }
</style>
