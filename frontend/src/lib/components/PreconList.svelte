<script lang="ts">
	/**
	 * The preconstructed decks, under a heading per set, in the order the server
	 * lists them (newest set first). A refused repeat is put to the user on its
	 * own row.
	 */
	import { count } from '$lib/format';
	import { Button } from '$lib/components/ui';
	import type { Precon } from '$lib/types/Precon';

	let {
		precons,
		busy = false,
		repeat = null,
		onAdd,
		onConfirm,
		onCancel
	}: {
		precons: Precon[];
		busy?: boolean;
		repeat?: { id: string; message: string } | null;
		onAdd: (id: string) => void;
		onConfirm: (id: string) => void;
		onCancel: () => void;
	} = $props();

	const sets = $derived.by(() => {
		const by = new Map<string, Precon[]>();
		for (const p of precons) {
			const rows = by.get(p.set_name);
			if (rows) rows.push(p);
			else by.set(p.set_name, [p]);
		}
		return [...by];
	});
</script>

{#each sets as [set, rows] (set)}
	<section>
		<h2>{set}</h2>
		<ul>
			{#each rows as p (p.precon_id)}
				<li data-precon={p.precon_id}>
					{#if p.leader_image}
						<img src={p.leader_image} alt="" loading="lazy" />
					{:else}
						<span class="noart" aria-hidden="true"></span>
					{/if}
					<div class="what">
						<span class="name">{p.deck ? `${p.name} (${p.deck})` : p.name}</span>
						<span class="meta">{count(p.card_count)} cards</span>
					</div>
					<Button disabled={busy} onclick={() => onAdd(p.precon_id)}>
						Add to collection and create deck
					</Button>
					{#if repeat?.id === p.precon_id}
						<div class="repeat" role="alert">
							<span>{repeat.message}</span>
							<Button disabled={busy} onclick={() => onConfirm(p.precon_id)}>
								Add a second copy
							</Button>
							<Button variant="link" disabled={busy} onclick={onCancel}>Cancel</Button>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/each}

<style>
	h2 {
		color: var(--color-text-accent);
		font-size: var(--text-xl);
		margin: var(--space-4) 0 var(--space-2);
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-3);
		background: var(--color-surface-panel);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-xl);
		padding: var(--space-2) var(--space-3);
	}
	img,
	.noart {
		width: 5rem;
		height: auto;
		aspect-ratio: 7 / 5;
		object-fit: contain;
		border-radius: var(--radius-md);
	}
	.noart {
		background: var(--color-surface-page);
	}
	.what {
		display: flex;
		flex-direction: column;
		flex: 1 1 12rem;
		min-width: 0;
	}
	.name {
		font-weight: var(--weight-medium);
	}
	.meta {
		font-size: var(--text-md);
		color: var(--color-text-subtle);
	}
	.repeat {
		flex-basis: 100%;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
		color: var(--color-text-accent);
	}
</style>
