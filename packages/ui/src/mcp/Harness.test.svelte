<script lang="ts">
  // Test-only host root for McpClientSettings.
  import { setKitContext, type KitContext } from '../context'
  import McpClientSettings from './McpClientSettings.svelte'

  let { kit }: { kit: KitContext } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<McpClientSettings />
