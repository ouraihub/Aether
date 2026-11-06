<script lang="ts">
  // A 24×24 single-path mark colored by `currentColor`, for marks absent from the icon library.
  import type { SVGAttributes } from 'svelte/elements'

  let { d, ...rest }: { d: string } & SVGAttributes<SVGSVGElement> = $props()
</script>

<svg viewBox="0 0 24 24" aria-hidden="true" {...rest}><path fill="currentColor" {d} /></svg>
