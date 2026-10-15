<script lang="ts">
	import { rarityMarks } from '$lib/rarity.svelte';

	let { name }: { name: string | null | undefined } = $props();
	const mark = $derived(rarityMarks.get(name));

	$effect(() => {
		void rarityMarks.load();
	});
</script>

{#if name}
	{#if mark}
		<span class="rarity-mark" role="img" aria-label={mark.name} title={mark.name} style:color={mark.color}
			>{mark.character}</span
		>
	{:else}
		<span class="rarity-name">{name}</span>
	{/if}
{/if}

<style>
	.rarity-mark {
		display: inline-block;
		font-weight: 700;
		min-width: 1.1em;
		text-align: center;
		/* A thin halo keeps the black Special mark legible on a dark theme. */
		text-shadow: 0 0 1px var(--color-border);
	}
</style>
