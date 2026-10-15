<script lang="ts">
	import { aspectOf, aspectIconUrl } from '$lib/aspects';

	let { name, size = '1.25em' }: { name: string; size?: string } = $props();
	const aspect = $derived(aspectOf(name));
</script>

{#if aspect}
	<img
		class="aspect-icon"
		src={aspectIconUrl(aspect)}
		alt={aspect.name}
		title={aspect.name}
		width="150"
		height="150"
		style:width={size}
		style:height={size}
	/>
{:else}
	<span class="aspect-name">{name}</span>
{/if}

<style>
	.aspect-icon {
		display: inline-block;
		vertical-align: middle;
		object-fit: contain;
	}
</style>
