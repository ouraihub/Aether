<script lang="ts">
  // Test-only host root: sets the kit context around ChatPanel.
  import { setKitContext, type KitContext } from '../context'
  import type { ModelRef } from '../ports'
  import ChatPanel from './ChatPanel.svelte'

  let { kit, model }: { kit: KitContext; model: ModelRef } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<ChatPanel {model} />
