<script lang="ts">
  // Test-only host root: sets the kit context around ChatPanel.
  import { setKitContext, type KitContext } from '../context'
  import type { ModelRef } from '../ports'
  import ChatPanel from './ChatPanel.svelte'

  let {
    kit,
    model,
    context = null,
  }: { kit: KitContext; model: ModelRef; context?: { kind: string; id: string; label: string } | null } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<ChatPanel {model} {context} />
