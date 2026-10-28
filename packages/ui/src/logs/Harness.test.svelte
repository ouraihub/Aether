<script lang="ts">
  import { setKitContext, type KitContext } from '../context'
  import LogPanel from './LogPanel.svelte'
  let { kit }: { kit: KitContext } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
</script>

<LogPanel />
