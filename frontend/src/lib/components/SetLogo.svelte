<script lang="ts">
	import { loadSetLogos, setLogo } from '$lib/setLogos.svelte';

	let { code, name = '' }: { code: string; name?: string } = $props();

	loadSetLogos();
	const src = $derived(setLogo(code));
</script>

{#if src}
	<img class="setmark" {src} alt="{name || code} logo" loading="lazy" />
{/if}

<style>
	.setmark {
		height: 1.4em;
		width: auto;
		max-width: 4.5em;
		object-fit: contain;
		vertical-align: middle;
		margin-inline-end: var(--space-1, 0.25rem);
	}
</style>
