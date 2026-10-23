<script lang="ts">
	import { Badge, Field } from '../kit/ui/index.ts';
	import type { Deck } from '$lib/types/Deck';
	import type { DeckGroup } from '$lib/types/DeckGroup';

	let {
		deck,
		groups,
		states,
		busy,
		onState,
		onGroup
	}: {
		deck: Pick<Deck, 'owner' | 'format' | 'state' | 'group'>;
		groups: DeckGroup[];
		states: string[];
		busy: boolean;
		onState: (state: string) => void;
		onGroup: (group: string) => void;
	} = $props();

	const group = $derived(groups.find((g) => g.name === deck.group));
</script>

<div class="meta">
	{#if deck.owner}<Badge tone="neutral">{deck.owner}</Badge>{/if}
	{#if deck.format}<Badge tone="accent">{deck.format}</Badge>{/if}
	<Field
		as="select"
		label="Lifecycle"
		value={deck.state}
		disabled={busy}
		onchange={(e: Event) => onState((e.currentTarget as HTMLSelectElement).value)}
	>
		{#each states as s (s)}<option value={s}>{s}</option>{/each}
	</Field>
	<Field
		as="select"
		label="Group"
		value={deck.group ?? ''}
		disabled={busy}
		onchange={(e: Event) => onGroup((e.currentTarget as HTMLSelectElement).value)}
	>
		<option value="">None</option>
		{#each groups as g (g.id)}<option value={g.name}>{g.name}</option>{/each}
	</Field>
	{#if group}
		<a class="group-link" href="/deck-groups/{group.id}">
			<Badge tone="info" variant="outline">Open group: {group.name} →</Badge>
		</a>
	{/if}
</div>

<style>
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-end;
		gap: var(--space-2) var(--space-3);
		margin-top: var(--space-2);
	}
	.group-link {
		text-decoration: none;
		max-width: 100%;
	}
	.group-link:hover {
		text-decoration: underline;
	}
</style>
