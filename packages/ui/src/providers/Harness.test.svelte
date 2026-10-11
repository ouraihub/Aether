<script lang="ts">
  // Test-only host root: sets the kit context around ProvidersSettings.
  import { setKitContext, type KitContext } from '../context'
  import ProvidersSettings from './ProvidersSettings.svelte'

  let { kit }: { kit: KitContext } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<ProvidersSettings />
