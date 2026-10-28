<script lang="ts">
  // Test-only host root for MCP settings.
  import { setKitContext, type KitContext } from '../context'
  import McpClientSettings from './McpClientSettings.svelte'
  import McpServerSettings from './McpServerSettings.svelte'

  let {
    kit,
    mode = 'client',
    serverName = 'aether',
  }: { kit: KitContext; mode?: 'client' | 'server'; serverName?: string } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

{#if mode === 'server'}
  <McpServerSettings {serverName} />
{:else}
  <McpClientSettings />
{/if}
