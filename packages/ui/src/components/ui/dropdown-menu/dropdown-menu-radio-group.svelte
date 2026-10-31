<!-- Local change: `value as never` like select.svelte,
  as exactOptionalPropertyTypes rejects the bound `string | undefined` " +
    "(docs/04 §4). -->
<script lang="ts">
	import { DropdownMenu as DropdownMenuPrimitive } from "bits-ui";

	let {
		ref = $bindable(null),
		value = $bindable(),
		...restProps
	}: DropdownMenuPrimitive.RadioGroupProps = $props();
</script>

<DropdownMenuPrimitive.RadioGroup
	bind:ref
	bind:value={value as never}
	data-slot="dropdown-menu-radio-group"
	{...restProps}
/>
