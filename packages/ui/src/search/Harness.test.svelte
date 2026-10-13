<script lang="ts">
  import { setKitContext, type KitContext } from '../context'
  import SearchSettings from './SearchSettings.svelte'
  let { kit }: { kit: KitContext } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<SearchSettings />
