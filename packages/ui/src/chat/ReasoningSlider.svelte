<script lang="ts">
  // The one discrete effort rail: a tone gradient fill and a pill thumb carrying the level's
  // mark and name over a native range input (docs/16, 31).
  import type { ReasoningEffort } from '../ports'
  import { effortTone } from './model-effort'
  import PathIcon from './icons/PathIcon.svelte'
  import { effortPath } from './icons/effort-icons'

  let {
    efforts,
    value,
    defaultIndex = 0,
    disabled = false,
    label,
    valueText,
    onchange,
  }: {
    /** Declared levels in display order; needs at least two. */
    efforts: readonly ReasoningEffort[]
    value: ReasoningEffort
    /** Visual position for `default`; never written back. */
    defaultIndex?: number
    disabled?: boolean
    label: string
    valueText: string
    onchange: (effort: ReasoningEffort) => void
  } = $props()
  const found = $derived(efforts.indexOf(value))
  const index = $derived(found >= 0 ? found : defaultIndex)
  const last = $derived(Math.max(1, efforts.length - 1))
  const tone = $derived(effortTone(found >= 0 ? value : 'default'))
  const TONE_COLOR = {
    fast: 'var(--color-info)',
    regular: 'var(--color-primary)',
    deep: 'var(--color-warning)',
  } as const
  const fill = $derived(
    `linear-gradient(to right, ${efforts
      .map((effort, at) => `${TONE_COLOR[effortTone(effort)]} ${(at / last) * 100}%`)
      .join(', ')})`,
  )
  function commit(event: Event & { currentTarget: HTMLInputElement }) {
    const effort = efforts[Number(event.currentTarget.value)]
    if (effort && !disabled) onchange(effort)
  }
</script>

<div
  class="reasoning-slider"
  data-tone={tone}
  style:--reasoning-fraction={index / last}
  style:--reasoning-fill={fill}
>
  <div class="reasoning-track" aria-hidden="true"><span class="reasoning-fill"></span></div>
  <input
    type="range"
    min="0"
    max={efforts.length - 1}
    step="1"
    value={index}
    aria-label={label}
    aria-valuetext={valueText}
    {disabled}
    oninput={commit}
    onpointerup={(event) => {
      if (event.button === 0) commit(event)
    }}
    onkeyup={(event) => {
      if (
        !event.isComposing &&
        ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)
      )
        commit(event)
    }}
  />
  <span class="reasoning-thumb" aria-hidden="true"
    ><PathIcon d={effortPath(found >= 0 ? value : 'default')} class="reasoning-icon" /><span
      >{valueText}</span
    ></span
  >
</div>

<style>
  /* The native thumb is as wide as the pill, so the range maps 1:1 onto the inset track. */
  .reasoning-slider {
    --reasoning-color: var(--color-primary);
    --reasoning-pill: 4.25rem;
    position: relative;
    height: 2rem;
  }
  .reasoning-slider[data-tone='fast'] {
    --reasoning-color: var(--color-info);
  }
  .reasoning-slider[data-tone='deep'] {
    --reasoning-color: var(--color-warning);
  }
  .reasoning-track {
    position: absolute;
    top: 50%;
    inset-inline: calc(var(--reasoning-pill) / 2);
    height: 0.75rem;
    overflow: hidden;
    border: 1px solid var(--color-border);
    border-radius: 999px;
    background: var(--color-secondary, var(--color-muted));
    transform: translateY(-50%);
  }
  .reasoning-fill {
    position: absolute;
    inset: 0;
    background: var(--reasoning-fill);
    clip-path: inset(0 calc((1 - var(--reasoning-fraction)) * 100%) 0 0);
  }
  .reasoning-thumb {
    position: absolute;
    top: 50%;
    left: calc(
      var(--reasoning-pill) / 2 + (100% - var(--reasoning-pill)) * var(--reasoning-fraction)
    );
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.25rem;
    box-sizing: border-box;
    width: var(--reasoning-pill);
    height: 1.5rem;
    border: 1px solid var(--color-border);
    border-radius: 999px;
    background: var(--color-popover);
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.08);
    color: var(--color-foreground);
    font-size: var(--kit-font-size-xs, 0.75rem);
    white-space: nowrap;
    pointer-events: none;
    transform: translate(-50%, -50%);
  }
  .reasoning-thumb :global(.reasoning-icon) {
    width: 0.875rem;
    height: 0.875rem;
    flex-shrink: 0;
    color: var(--reasoning-color);
  }
  input[type='range'] {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    appearance: none;
    background: transparent;
    opacity: 0;
    cursor: pointer;
  }
  input::-webkit-slider-thumb {
    appearance: none;
    width: var(--reasoning-pill);
    height: 1.5rem;
  }
  input::-moz-range-thumb {
    width: var(--reasoning-pill);
    height: 1.5rem;
    border: 0;
  }
  input:disabled {
    cursor: not-allowed;
  }
  .reasoning-slider:has(input:disabled) {
    opacity: 0.5;
  }
  .reasoning-slider:has(input:focus-visible) .reasoning-thumb {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
</style>
