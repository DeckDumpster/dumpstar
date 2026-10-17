<!--
	TraitLinks — the collection Traits cell: each trait its own trait-search
	link, separated by a middle dot, wrapping only after a separator.
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

{#each list as t, i (i)}{#if i > 0}{' '}{/if}<span class="unit"><a
			class="t-trait"
			href={facetHref('trait', t)}
			title="Find all {t} cards"
			onclick={onlink}>{t}</a
		>{#if i < list.length - 1}<span class="sep">&nbsp;&middot;</span>{/if}</span
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
	/* One trait and the dot after it: lines break between units (the plain
	   space between them), never inside a name unless it is wider than the cell. */
	.unit {
		display: inline-block;
		max-width: 100%;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.sep {
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
	}
</style>
