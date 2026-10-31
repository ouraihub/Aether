<script lang="ts">
  // Test-only host root: sets the kit context around ChatPanel.
  import type { ComponentProps } from 'svelte'
  import { setKitContext, type KitContext } from '../context'
  import ChatPanel from './ChatPanel.svelte'

  let {
    kit,
    ...props
  }: { kit: KitContext } & ComponentProps<typeof ChatPanel> = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<ChatPanel {...props} />
