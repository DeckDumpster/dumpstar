<!--
	TypeLines — the collection Type cell's content, as the deck table shows it:
	the card type as a search link on the first line, its arena(s) beneath in the
	smaller, dimmer secondary style (`Unit` over `Ground`). A card with no arena
	shows only the type. `onlink` lets the row decide what a click does (select
	mode keeps the row's own selection click).
-->
<script lang="ts">
	import { facetHref } from '$lib/facets';
	import { jsonList, typeLines } from '$lib/format';

	let {
		cardType,
		arenas,
		onlink
	}: {
		cardType: string | null | undefined;
		arenas: string | null | undefined;
		onlink?: (e: MouseEvent) => void;
	} = $props();
	const lines = $derived(typeLines(cardType, arenas));
	const arenaList = $derived(jsonList(arenas));
</script>

{#if lines.type}<a class="t-type" href={facetHref('type', lines.type)} title="Find all {lines.type} cards" onclick={onlink}>{lines.type}</a>{/if}{#if arenaList.length}<span class="t-sub">{#each arenaList as a, i (i)}{#if i > 0}{' \u00b7 '}{/if}<a class="t-arena" href={facetHref('arena', a)} title="Find all {a} cards" onclick={onlink}>{a}</a>{/each}</span>{/if}

<style>
	.t-type,
	.t-sub {
		display: block;
		overflow-wrap: anywhere;
	}
	.t-type {
		color: inherit;
		text-decoration: underline;
		text-decoration-color: var(--color-accent);
	}
	.t-arena {
		color: inherit;
		text-decoration: underline;
		text-decoration-color: var(--color-accent);
	}
	.t-sub {
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
	}
</style>
