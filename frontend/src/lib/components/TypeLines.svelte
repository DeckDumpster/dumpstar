<!--
	TypeLines — the collection Type cell's content, as the deck table shows it:
	the card type as a search link on the first line, its arena(s) beneath in the
	smaller, dimmer secondary style (`Unit` over `Ground`). A card with no arena
	shows only the type. `onlink` lets the row decide what a click does (select
	mode keeps the row's own selection click).
-->
<script lang="ts">
	import { facetHref } from '$lib/facets';
	import { typeLines } from '$lib/format';

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
</script>

{#if lines.type}<a class="t-type" href={facetHref('type', lines.type)} title="Find all {lines.type} cards" onclick={onlink}>{lines.type}</a>{/if}{#if lines.arena}<span class="t-sub">{lines.arena}</span>{/if}

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
	.t-sub {
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
	}
</style>
