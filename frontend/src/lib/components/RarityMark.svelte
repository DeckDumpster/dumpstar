<script lang="ts">
	import { rarityMarks } from '$lib/rarity.svelte';
	import { rarityShape } from '$lib/rarityShapes';

	let { name }: { name: string | null | undefined } = $props();
	const mark = $derived(rarityMarks.get(name));
	const shape = $derived(name ? rarityShape(name) : null);

	$effect(() => {
		void rarityMarks.load();
	});
</script>

{#if name && shape}
	<span class="rarity-mark" role="img" aria-label={name} title={name} style:color={mark?.color ?? shape.color}>
		<svg viewBox="0 0 25 25" aria-hidden="true" focusable="false">
			{#if shape.frame}<path d={shape.frame} fill="var(--color-text-strong)" />{/if}
			<path d={shape.path} fill="currentColor" fill-rule="evenodd" />
		</svg>
	</span>
{/if}

<style>
	.rarity-mark {
		display: inline-flex;
		vertical-align: middle;
	}
	.rarity-mark svg {
		width: 1.1em;
		height: 1.1em;
	}
</style>
