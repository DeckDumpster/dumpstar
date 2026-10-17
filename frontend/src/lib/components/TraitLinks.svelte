<!--
	TraitLinks — the collection Traits cell: each trait its own trait-search
	link, separated by a middle dot, wrapping only between traits' words.
-->
<script lang="ts">
	import { facetHref } from '$lib/facets';
	import { jsonList } from '$lib/format';

	let {
		traits,
		onlink
	}: { traits: string | null | undefined; onlink?: (e: MouseEvent) => void } = $props();
	const list = $derived(jsonList(traits));
</script>

{#each list as t, i (i)}{#if i > 0}<span class="sep"> &middot; </span>{/if}<a
		class="t-trait"
		href={facetHref('trait', t)}
		title="Find all {t} cards"
		onclick={onlink}>{t}</a
	>{/each}

<style>
	.t-trait {
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
		text-decoration: underline;
		text-decoration-color: var(--color-accent);
		overflow-wrap: normal;
		word-break: normal;
	}
	.sep {
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
	}
</style>
