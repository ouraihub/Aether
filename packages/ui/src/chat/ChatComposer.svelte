<script lang="ts">
  import { tick, untrack, type Snippet } from 'svelte'
  import MaximizeIcon from '@lucide/svelte/icons/maximize-2'
  import MinimizeIcon from '@lucide/svelte/icons/minimize-2'
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up'
  import SquareIcon from '@lucide/svelte/icons/square'
  import { Button } from '#lib/components/ui/button/index.js'

  let {
    value = $bindable(''),
    label,
    placeholder,
    submitLabel,
    stopLabel = 'Stop',
    disabled = false,
    running = false,
    stopping = false,
    maxlength,
    onsubmit,
    onstop,
    oninput,
    onpaste,
    onfiles,
    top,
    tools,
    actions,
    override,
    expandLabel = 'Expand input',
    collapseLabel = 'Collapse input',
  }: {
    value?: string | undefined
    label: string
    placeholder?: string | undefined
    submitLabel: string
    stopLabel?: string | undefined
    disabled?: boolean | undefined
    running?: boolean | undefined
    stopping?: boolean | undefined
    maxlength?: number | undefined
    onsubmit: () => void
    onstop?: (() => void) | undefined
    oninput?: (() => void) | undefined
    onpaste?: ((event: ClipboardEvent) => void) | undefined
    onfiles?: ((files: File[]) => void) | undefined
    top?: Snippet | undefined
    tools?: Snippet | undefined
    actions?: Snippet | undefined
    override?: Snippet | undefined
    expandLabel?: string | undefined
    collapseLabel?: string | undefined
  } = $props()
  let expanded = $state(false)
  let textarea = $state<HTMLTextAreaElement | null>(null)
  let normal = $state<HTMLFormElement | null>(null)
  let replacement = $state<HTMLDivElement | null>(null)
  let restoreFocus: HTMLElement | null = null
  let overridden = false
  function submit() {
    if (!disabled && !running && !override) onsubmit()
  }
  function resize() {
    if (!textarea) return
    textarea.style.height = 'auto'
    textarea.style.height = `${textarea.scrollHeight}px`
  }
  $effect(() => {
    void value
    void expanded
    void override
    void tick().then(resize)
  })
  $effect.pre(() => {
    const next = !!override
    untrack(() => {
      if (next && !overridden) {
        const active = document.activeElement
        restoreFocus = active instanceof HTMLElement && normal?.contains(active) ? active : null
      } else if (!next && overridden) {
        const active = document.activeElement
        const shouldRestore =
          active === document.body || !!(active && replacement?.contains(active))
        const target = restoreFocus
        void tick().then(() => {
          if (
            shouldRestore &&
            target?.isConnected &&
            (document.activeElement === document.body || document.activeElement === active)
          )
            target.focus({ preventScroll: true })
        })
        restoreFocus = null
      }
      overridden = next
    })
  })
</script>

<div class="composer-shell">
  <form
    class="composer"
    class:expanded
    bind:this={normal}
    hidden={!!override}
    inert={!!override}
    onsubmit={(event) => {
      event.preventDefault()
      submit()
    }}
    ondragover={(event) => {
      if (onfiles && event.dataTransfer?.types.includes('Files')) event.preventDefault()
    }}
    ondrop={(event) => {
      if (!onfiles || !event.dataTransfer?.files.length) return
      event.preventDefault()
      onfiles([...event.dataTransfer.files])
    }}
  >
    <div class="expand-control">
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label={expanded ? collapseLabel : expandLabel}
        title={expanded ? collapseLabel : expandLabel}
        aria-expanded={expanded}
        onmousedown={(event) => event.preventDefault()}
        onclick={() => {
          expanded = !expanded
        }}
      >
        {#if expanded}<MinimizeIcon />{:else}<MaximizeIcon />{/if}
      </Button>
    </div>
    {#if top}<div class="composer-top">{@render top()}</div>{/if}
    <textarea
      bind:this={textarea}
      bind:value
      rows="2"
      aria-label={label}
      {placeholder}
      {maxlength}
      oninput={() => {
        resize()
        oninput?.()
      }}
      {onpaste}
      onkeydown={(event) => {
        if (event.key === 'Escape' && expanded && !event.isComposing && !event.defaultPrevented) {
          event.preventDefault()
          event.stopPropagation()
          expanded = false
          return
        }
        if (
          event.key === 'Enter' &&
          (event.ctrlKey || event.metaKey) &&
          !event.isComposing &&
          event.keyCode !== 229
        ) {
          event.preventDefault()
          if (!event.repeat) submit()
        }
      }}></textarea>
    <div class="composer-tools">
      <div class="tool-group">
        {#if tools}{@render tools()}{/if}
      </div>
      <div class="tool-group send-tools">
        {#if actions}{@render actions()}{/if}
        {#if running && onstop}<Button
            type="button"
            variant="destructive"
            size="icon-sm"
            disabled={stopping}
            aria-label={stopLabel}
            title={stopLabel}
            onclick={onstop}><SquareIcon /></Button
          >
        {:else}<Button
            type="submit"
            size="icon-sm"
            disabled={disabled || running}
            aria-label={submitLabel}
            title={submitLabel}><ArrowUpIcon /></Button
          >{/if}
      </div>
    </div>
  </form>
  {#if override}<div class="composer-override" bind:this={replacement}>
      {@render override()}
    </div>{/if}
</div>

<style>
  /* Hallmark · component: integrated composer · theme: host semantic tokens */
  .composer-shell {
    min-width: 0;
    container-type: inline-size;
    container-name: composer;
  }
  .composer {
    box-sizing: border-box;
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-width: 0;
    max-height: 68cqh;
    overflow-y: auto;
    border: 1px solid var(--color-border);
    border-radius: calc(var(--kit-radius, var(--radius)) * 1.5);
    padding: 0.5rem;
    background: var(--color-background);
  }
  .composer[hidden] {
    display: none;
  }
  .expand-control {
    position: absolute;
    top: 0.25rem;
    right: 0.25rem;
  }
  @media (pointer: coarse) {
    .expand-control :global(button),
    .composer-tools :global(button) {
      min-width: 44px;
      min-height: 44px;
    }
  }
  .composer-top {
    padding-right: 2.75rem;
    min-width: 0;
  }
  textarea {
    box-sizing: border-box;
    display: block;
    flex-shrink: 0;
    width: 100%;
    min-width: 0;
    min-height: 3.5rem;
    max-height: 35cqh;
    resize: none;
    padding: 0.5rem 2.75rem 0.5rem 0.5rem;
    border: 0;
    border-radius: var(--kit-radius-sm, var(--radius));
    background: transparent;
    color: var(--color-foreground);
    font: inherit;
    line-height: 1.5;
    overflow-y: auto;
  }
  textarea::placeholder {
    color: var(--color-muted-foreground);
  }
  textarea:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }
  .expanded textarea {
    min-height: 55cqh;
    max-height: 55cqh;
  }
  .composer-tools,
  .tool-group {
    display: flex;
    align-items: center;
    flex-wrap: nowrap;
    gap: 0.25rem;
    min-width: 0;
  }
  .composer-tools {
    justify-content: space-between;
    gap: 0.5rem;
    overflow: visible;
  }
  /* Tools then the model pill on the left; only the pill shrinks. Usage and send stay whole. */
  .tool-group:first-child {
    flex: 1 1 auto;
  }
  .tool-group > :global(:not([data-composer-model])) {
    flex-shrink: 0;
  }
  .send-tools {
    flex: 0 0 auto;
    justify-content: flex-end;
    margin-left: auto;
  }
  @container composer (max-width: 560px) {
    .composer-tools :global([data-composer-mode-label]),
    .composer-tools :global([data-composer-model-detail]),
    .composer-tools :global([data-composer-capacity]) {
      display: none;
    }
    .composer-tools :global([data-composer-model]) {
      flex: 0 1 auto;
      min-width: 4.5rem;
      max-width: 9rem;
      padding-inline: 2px;
    }
    .composer-tools {
      gap: 2px;
    }
  }
  @container composer (max-width: 400px) {
    .composer-tools :global([data-composer-model]) {
      max-width: 6rem;
    }
    .composer-tools :global([data-slot='select-trigger']) {
      max-width: 5.5rem;
      overflow: hidden;
    }
    .composer-tools :global([data-composer-model] [data-icon='inline-end']) {
      display: none;
    }
  }
  .composer-override {
    min-width: 0;
  }
</style>
