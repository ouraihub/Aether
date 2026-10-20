<script lang="ts">
  // Host-owned model setting (docs/16 §4.1): the value is stored by the app; a deleted model shows as invalid.
  // Cherry-style picker: search, capability filters, provider groups (label up to its last ` · `).
  import { Command, Popover } from 'bits-ui'
  import EyeIcon from '@lucide/svelte/icons/eye'
  import BrainIcon from '@lucide/svelte/icons/brain'
  import WrenchIcon from '@lucide/svelte/icons/wrench'
  import GlobeIcon from '@lucide/svelte/icons/globe'
  import SearchIcon from '@lucide/svelte/icons/search'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import { Button } from '#lib/components/ui/button/index.js'
  import { getKitContext } from '../context'
  import type { Snippet } from 'svelte'
  import type { ModelFilter, ModelOption, ModelRef } from '../ports'
  import { providersRevision } from './revision.svelte'

  let {
    value,
    filter = {},
    onchange,
    placeholder,
    disabled = false,
    options: suppliedOptions,
    onOpenSettings,
    trigger,
  }: {
    value: ModelRef | null
    filter?: ModelFilter
    onchange: (value: ModelRef, option: ModelOption | null) => void
    placeholder?: string
    disabled?: boolean
    /** Reuses capabilities already loaded by a containing panel; null means loading. */
    options?: ModelOption[] | null
    /** Shows a footer link to the model settings. */
    onOpenSettings?: () => void
    /** Replaces the default trigger button; spread `props` onto the element. */
    trigger?: Snippet<[{ props: Record<string, unknown> }]>
  } = $props()

  const { ports, i18n } = getKitContext()
  const { t } = i18n
  let loadedOptions = $state<ModelOption[] | null>(null)
  let failed = $state(false)
  let reload = $state(0)
  let open = $state(false)
  let highlighted = $state('')
  let active = $state<Cap[]>([])
  const options = $derived(suppliedOptions === undefined ? loadedOptions : suppliedOptions)

  const key = (r: ModelRef) => `${r.endpointId}\n${r.modelId}`
  // label is `{provider} · {slug} · {modelId}`; a model's display name never enters it.
  const group = (o: ModelOption) => o.label.slice(0, Math.max(0, o.label.lastIndexOf(' · '))) || o.label
  type Cap = 'vision' | 'reasoning' | 'tools' | 'web'
  const CAPS = [
    { cap: 'vision', icon: EyeIcon, has: (o: ModelOption) => o.canVision },
    { cap: 'reasoning', icon: BrainIcon, has: (o: ModelOption) => o.reasoning !== null },
    { cap: 'tools', icon: WrenchIcon, has: (o: ModelOption) => o.supportsTools === true },
    { cap: 'web', icon: GlobeIcon, has: (o: ModelOption) => o.nativeWebSearch },
  ] as const
  const caps = $derived(CAPS.filter((c) => options?.some(c.has)))
  const groups = $derived.by(() => {
    const out: [string, ModelOption[]][] = []
    for (const o of options ?? []) {
      if (!CAPS.every((c) => !active.includes(c.cap) || c.has(o))) continue
      const name = group(o)
      const found = out.find(([n]) => n === name)
      if (found) found[1].push(o)
      else out.push([name, [o]])
    }
    return out
  })
  const match = (_: string, search: string, keywords: string[] = []) =>
    keywords.join(' ').toLowerCase().includes(search.trim().toLowerCase()) ? 1 : 0

  $effect(() => {
    if (suppliedOptions !== undefined) return
    void reload
    void providersRevision.value
    loadedOptions = null
    failed = false
    let alive = true
    void ports.providers
      ?.modelOptions(filter)
      .then((list) => {
        if (alive) loadedOptions = list
      })
      .catch(() => {
        if (alive) failed = true
      })
    return () => {
      alive = false
    }
  })
  const current = $derived(value && options?.find((o) => key(o) === key(value)))
  const invalid = $derived(!failed && value !== null && options !== null && !current)

  function pick(option: ModelOption) {
    open = false
    onchange({ endpointId: option.endpointId, modelId: option.modelId }, option)
  }
</script>

{#if ports.providers}
  {#if failed}
    <div class="model-error" role="alert">
      <span>{t('chat.modelsFailed')}</span>
      <Button variant="outline" size="sm" {disabled} onclick={() => reload++}>{t('chat.retry')}</Button>
    </div>
  {:else}
    <Popover.Root
      bind:open
      onOpenChange={(next) => {
        if (next) highlighted = value ? key(value) : ''
      }}
    >
      <Popover.Trigger disabled={disabled || options === null || options.length === 0}>
        {#snippet child({ props })}
          {#if trigger}{@render trigger({ props })}{:else}
            <Button
              {...props}
              variant="outline"
              class="model-trigger"
              aria-invalid={invalid}
              aria-label={t('providers.model_placeholder')}
              title={current?.label}
            >
              {#if options === null}
                <span class="model-name muted">{t('chat.modelsLoading')}</span>
              {:else if current}
                <span class="model-name">{current.name ?? current.modelId}</span>
                <span class="model-group">| {group(current)}</span>
              {:else}
                <span class="model-name muted"
                  >{invalid ? t('providers.model_invalid') : (placeholder ?? t('providers.model_placeholder'))}</span
                >
              {/if}
              <ChevronDownIcon class="model-chevron" />
            </Button>
          {/if}
        {/snippet}
      </Popover.Trigger>
      <Popover.Content
        class="model-popover"
        side="bottom"
        align="start"
        sideOffset={4}
        collisionPadding={8}
        onEscapeKeydown={(event) => {
          if (event.isComposing) event.preventDefault()
        }}
      >
        <Command.Root bind:value={highlighted} filter={match} label={t('providers.model_placeholder')}>
          <div class="model-search">
            <SearchIcon class="model-search-icon" />
            <Command.Input class="model-search-input" placeholder={t('providers.model_search')} />
          </div>
          {#if caps.length > 0}
            <div class="model-caps">
              {#each caps as c (c.cap)}
                <button
                  type="button"
                  class="cap-chip cap-{c.cap}"
                  aria-pressed={active.includes(c.cap)}
                  onclick={() =>
                    (active = active.includes(c.cap) ? active.filter((x) => x !== c.cap) : [...active, c.cap])}
                  ><c.icon />{t(`providers.cap_${c.cap}`)}</button
                >
              {/each}
            </div>
          {/if}
          <Command.List class="model-list" aria-label={t('providers.model_placeholder')}>
            <Command.Empty class="model-empty">{t('providers.model_no_match')}</Command.Empty>
            {#each groups as [name, items] (name)}
              <Command.Group value={`group\n${name}`}>
                <Command.GroupHeading class="model-heading">{name}</Command.GroupHeading>
                <Command.GroupItems>
                  {#each items as option (key(option))}
                    <Command.Item
                      class="model-item"
                      value={key(option)}
                      keywords={[option.modelId, option.name ?? '', name]}
                      aria-current={value !== null && key(option) === key(value) ? 'true' : undefined}
                      onSelect={() => pick(option)}
                    >
                      <span class="model-avatar" aria-hidden="true"
                        >{(option.name ?? option.modelId).charAt(0).toUpperCase()}</span
                      >
                      <span class="model-name" title={option.label}>{option.name ?? option.modelId}</span>
                      <span class="model-tags">
                        {#each CAPS.filter((c) => c.has(option)) as c (c.cap)}
                          <span class="cap-tag cap-{c.cap}" title={t(`providers.cap_${c.cap}`)}
                            ><c.icon aria-label={t(`providers.cap_${c.cap}`)} /></span
                          >
                        {/each}
                      </span>
                    </Command.Item>
                  {/each}
                </Command.GroupItems>
              </Command.Group>
            {/each}
          </Command.List>
          {#if onOpenSettings}
            <button
              type="button"
              class="model-footer"
              onclick={() => {
                open = false
                onOpenSettings()
              }}><Settings2Icon />{t('providers.model_configure')}</button
            >
          {/if}
        </Command.Root>
      </Popover.Content>
    </Popover.Root>
  {/if}
{/if}

<style>
  .model-error {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    color: var(--color-muted-foreground);
  }
  :global(.model-trigger) {
    width: 100%;
    min-width: 0;
    justify-content: flex-start;
    gap: 0.375rem;
    font-weight: 400;
  }
  .model-name {
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .model-group {
    min-width: 0;
    flex: 1 999 0%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-muted-foreground);
    text-align: left;
  }
  .muted {
    color: var(--color-muted-foreground);
  }
  :global(.model-chevron) {
    margin-left: auto;
    flex-shrink: 0;
    opacity: 0.6;
  }
  :global(.model-popover) {
    z-index: 50;
    box-sizing: border-box;
    width: min(25rem, calc(100vw - 1rem));
    display: flex;
    flex-direction: column;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
    box-shadow: 0 4px 12px color-mix(in oklch, var(--color-foreground) 12%, transparent);
    font-size: var(--kit-font-size-sm, 0.8125rem);
    overflow: hidden;
  }
  .model-search {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--color-border);
  }
  :global(.model-search-icon) {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
    color: var(--color-muted-foreground);
  }
  :global(.model-search-input) {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    outline: none;
  }
  .model-caps {
    display: flex;
    gap: 0.375rem;
    overflow-x: auto;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--color-border);
    scrollbar-width: none;
  }
  .cap-chip {
    display: inline-flex;
    flex-shrink: 0;
    align-items: center;
    gap: 0.25rem;
    padding: 0.125rem 0.5rem;
    border: 1px solid var(--color-border);
    border-radius: 999px;
    background: transparent;
    color: var(--color-muted-foreground);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .cap-chip[aria-pressed='true'] {
    border-color: var(--cap);
    background: color-mix(in oklch, var(--cap) 15%, transparent);
    color: var(--cap);
  }
  .cap-chip:focus-visible,
  .model-footer:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }
  .cap-chip :global(svg),
  .cap-tag :global(svg),
  .model-footer :global(svg) {
    width: 0.875rem;
    height: 0.875rem;
  }
  .cap-vision {
    --cap: oklch(0.62 0.15 155);
  }
  .cap-reasoning {
    --cap: oklch(0.6 0.16 290);
  }
  .cap-tools {
    --cap: oklch(0.68 0.16 55);
  }
  .cap-web {
    --cap: oklch(0.62 0.14 235);
  }
  :global(.model-list) {
    max-height: min(22rem, 50dvh);
    overflow-y: auto;
    padding: 0.25rem;
    scroll-padding-block: 1.75rem 0.25rem;
  }
  :global(.model-heading) {
    position: sticky;
    top: -0.25rem;
    z-index: 1;
    padding: 0.5rem 0.5rem 0.25rem;
    background: var(--color-popover);
    color: var(--color-muted-foreground);
    font-size: 0.75rem;
  }
  :global(.model-empty) {
    padding: 1.5rem 0.5rem;
    text-align: center;
    color: var(--color-muted-foreground);
  }
  :global(.model-item) {
    position: relative;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-height: 2.25rem;
    padding: 0.25rem 0.5rem;
    border-radius: calc(var(--kit-radius, var(--radius)) - 2px);
    cursor: pointer;
    user-select: none;
  }
  :global(.model-item[aria-current='true']:not([data-selected])) {
    background: color-mix(in oklch, var(--color-accent) 60%, transparent);
  }
  :global(.model-item[data-selected]) {
    background: var(--color-accent);
    color: var(--color-accent-foreground);
  }
  :global(.model-item[aria-current='true'])::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0.5rem;
    bottom: 0.5rem;
    width: 3px;
    border-radius: 3px;
    background: var(--color-primary);
  }
  .model-avatar {
    display: grid;
    flex-shrink: 0;
    place-items: center;
    width: 1.5rem;
    height: 1.5rem;
    border: 1px solid var(--color-border);
    border-radius: 999px;
    background: var(--color-muted);
    font-size: 0.6875rem;
    font-weight: 600;
  }
  .model-tags {
    display: flex;
    flex-shrink: 0;
    gap: 0.25rem;
    margin-left: auto;
  }
  .cap-tag {
    display: inline-grid;
    place-items: center;
    width: 1.5rem;
    height: 1.125rem;
    border-radius: 0.375rem;
    background: color-mix(in oklch, var(--cap) 15%, transparent);
    color: var(--cap);
  }
  .model-footer {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    border: 0;
    border-top: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-muted-foreground);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .model-footer:hover {
    color: var(--color-foreground);
  }
</style>
