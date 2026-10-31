<script lang="ts">
  // Test-only host root: sets the kit context around ProvidersSettings.
  import { setKitContext, type KitContext } from '../context'
  import type { ProviderPreset } from '../ports'
  import ProvidersSettings from './ProvidersSettings.svelte'

  let { kit, presets = [] }: { kit: KitContext; presets?: ProviderPreset[] } =
    $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<ProvidersSettings {presets} />
