<!--
	VariantBadges — one square letter per printing variant or finish beside a
	card name (S Showcase, H Hyperspace, F Foil …). The letters live in
	`printing-badges.json`; the full name is the tooltip and the aria-label.
-->
<script lang="ts">
	import { printingBadges } from '$lib/printing-badges.js';

	let { variant, finish }: { variant: string; finish: string } = $props();
	const list = $derived(printingBadges(variant, finish));
</script>

{#each list as b (b.letter)}<span class="vbadge" role="img" title={b.label} aria-label={b.label}
		>{b.letter}</span
	>{/each}

<style>
	.vbadge {
		display: inline-block;
		vertical-align: middle;
		margin-left: var(--space-1);
		min-width: 1.1em;
		padding: var(--space-px) var(--space-0-5);
		text-align: center;
		font-size: var(--text-xs);
		font-weight: var(--weight-semibold);
		line-height: var(--leading-tight);
		text-transform: uppercase;
		border-radius: var(--radius-xs);
		border: 1px solid var(--color-border);
		background: var(--color-surface-panel);
		color: var(--color-text-subtle);
		cursor: help;
	}
</style>
