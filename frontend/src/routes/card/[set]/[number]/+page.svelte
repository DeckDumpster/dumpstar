<script lang="ts">
	import { page } from '$app/state';
	import { breadcrumbs } from '$lib/breadcrumbs.svelte';
	import CardDetailView from '$lib/components/CardDetailView.svelte';

	const setCode = page.params.set ?? '';
	const number = page.params.number ?? '';

	// Set placeholder crumbs synchronously so first paint never shows the
	// "Card › sor › 5" URL-derived fallback (Card and sor both 404).
	// CardDetailView upgrades the middle and leaf labels to the full set
	// + card name once the API call resolves.
	if (setCode) {
		breadcrumbs.set([
			{ label: 'Browse', href: '/browse' },
			{ label: setCode, href: `/browse/${setCode}` },
			{ label: number ? `#${number}` : '' }
		]);
	}
</script>

<svelte:head><title>Card — Dumpstar</title></svelte:head>

{#if setCode && number}
	<CardDetailView {setCode} {number} manageBreadcrumbs />
{/if}
