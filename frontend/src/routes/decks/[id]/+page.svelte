<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import DeckLines from '$lib/components/DeckLines.svelte';
	import { buildDeckView } from '$lib/deckView';
	import CollectionPicker from '$lib/components/CollectionPicker.svelte';
	import { Button, EmptyState } from '$lib/components/ui';
	import type { DeckDetail } from '$lib/types/DeckDetail';

	let detail = $state<DeckDetail | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let picking = $state(false);
	let busy = $state(false);

	const states = ['idea', 'ready', 'built'];

	async function load() {
		const id = Number(page.params.id);
		if (!id) return;
		try {
			detail = await api.deckDetail(id);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void page.params.id;
		load();
	});

	async function changeState(state: string) {
		if (!detail) return;
		busy = true;
		error = null;
		try {
			await api.updateDeck(detail.deck.id, { state });
			await load();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	async function remove(copyId: number) {
		busy = true;
		error = null;
		try {
			await api.moveCopy(copyId, {}); // un-assign
			await load();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	async function removeDeck() {
		if (!detail) return;
		if (!confirm(`Delete deck "${detail.deck.name}"? Its cards stay in your collection.`)) return;
		try {
			await api.deleteDeck(detail.deck.id);
			goto('/decks');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}
</script>

<svelte:head><title>{detail ? detail.deck.name : 'Deck'} — Dumpstar</title></svelte:head>

{#if loading}
	<p class="muted">Loading…</p>
{:else if error && !detail}
	<p class="error">Failed to load deck: {error}</p>
{:else if detail}
	<header>
		<div>
			<h1>{detail.deck.name}</h1>
			<p class="sub">
				{#if detail.deck.owner}{detail.deck.owner} · {/if}
				{#if detail.deck.format}{detail.deck.format} · {/if}
				<label>
					Lifecycle
					<select
						value={detail.deck.state}
						disabled={busy}
						onchange={(e) => changeState(e.currentTarget.value)}
					>
						{#each states as s (s)}<option value={s}>{s}</option>{/each}
					</select>
				</label>
			</p>
		</div>
		<div class="actions">
			<button onclick={() => (picking = true)}>+ Add cards</button>
			<button class="danger" onclick={removeDeck}>Delete deck</button>
		</div>
	</header>

	{#if error}<p class="error">{error}</p>{/if}

	{#if detail.cards.length === 0}
		<EmptyState
			title="No cards in this deck."
			description="Pick copies you already own and they're assigned to this list, so the rest of the collection knows they're spoken for."
		>
			{#snippet action()}
				<Button onclick={() => (picking = true)}>Add cards</Button>
			{/snippet}
		</EmptyState>
	{:else}
		<DeckLines view={buildDeckView(detail.cards)} {busy} onRemove={remove} />
	{/if}
{/if}

{#if picking && detail}
	<CollectionPicker
		target={{ kind: 'deck', id: detail.deck.id, name: detail.deck.name }}
		onClose={() => (picking = false)}
		onAssigned={load}
	/>
{/if}

<style>
	header {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		flex-wrap: wrap;
		gap: 1rem;
	}
	h1 {
		color: var(--color-text-accent);
		margin: 0;
	}
	.sub {
		color: var(--color-text-subtle);
		font-size: 0.85rem;
		margin: 0.25rem 0 0;
	}
	.muted {
		color: var(--color-text-subtle);
	}
	.error {
		color: var(--color-text-accent);
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	select {
		background: var(--color-surface-page);
		border: 1px solid var(--color-border);
		color: var(--color-text);
		border-radius: 6px;
		padding: 0.15rem;
	}
	button {
		background: var(--color-accent);
		border: none;
		color: var(--color-on-accent);
		padding: 0.4rem 0.8rem;
		border-radius: 6px;
		cursor: pointer;
	}
	button.danger {
		background: var(--color-danger);
	}
</style>
