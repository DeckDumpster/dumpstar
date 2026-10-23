<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import { count } from '$lib/format';
	import { EmptyState } from '$lib/components/ui';
	import type { GroupDetail } from '$lib/types/GroupDetail';

	let detail = $state<GroupDetail | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let notice = $state<string | null>(null);
	let renaming = $state(false);
	let nameDraft = $state('');
	let descDraft = $state('');
	let formatDraft = $state('');
	let deleting = $state(false);

	async function load() {
		const id = Number(page.params.id);
		if (!id) return;
		try {
			detail = await api.deckGroup(id);
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

	function startEdit() {
		if (!detail) return;
		nameDraft = detail.group.name;
		descDraft = detail.group.description ?? '';
		formatDraft = detail.group.format ?? '';
		renaming = true;
	}

	async function run(work: () => Promise<void>) {
		busy = true;
		error = null;
		notice = null;
		try {
			await work();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	const save = () =>
		run(async () => {
			if (!detail || !nameDraft.trim()) return;
			detail = await api.updateDeckGroup(detail.group.id, {
				name: nameDraft.trim(),
				description: descDraft,
				format: formatDraft
			});
			renaming = false;
		});

	const releaseAll = () =>
		run(async () => {
			if (!detail) return;
			if (
				!confirm(
					`Release every deck in "${detail.group.name}"? Their cards return to the collection; the decks and their lists stay.`
				)
			)
				return;
			const { released } = await api.releaseDeckGroup(detail.group.id);
			notice = `Released ${count(released)} cards back to the collection.`;
			await load();
		});

	const removeGroup = (mode: 'ungroup' | 'delete') =>
		run(async () => {
			if (!detail) return;
			await api.deleteDeckGroup(detail.group.id, mode);
			goto('/decks');
		});
</script>

<svelte:head><title>{detail ? detail.group.name : 'Deck group'} — Dumpstar</title></svelte:head>

{#if loading}
	<p class="muted">Loading…</p>
{:else if error && !detail}
	<p class="error">Failed to load deck group: {error}</p>
{:else if detail}
	<p class="crumb"><a href="/decks">← Decks</a></p>
	<header>
		<div>
			<h1>{detail.group.name}</h1>
			<p class="sub">
				{#if detail.group.format}{detail.group.format} · {/if}
				{count(detail.group.deck_count)} decks · {count(detail.group.card_count)} cards
			</p>
			{#if detail.group.description}<p class="desc">{detail.group.description}</p>{/if}
		</div>
		<div class="actions">
			<button onclick={startEdit} disabled={busy}>Rename or describe</button>
			<button onclick={releaseAll} disabled={busy || detail.group.card_count === 0}>
				Release every deck
			</button>
			<button class="danger" onclick={() => (deleting = true)} disabled={busy}>
				Delete group
			</button>
		</div>
	</header>

	{#if error}<p class="error">{error}</p>{/if}
	{#if notice}<p class="muted">{notice}</p>{/if}

	{#if renaming}
		<form
			class="edit"
			onsubmit={(e) => {
				e.preventDefault();
				save();
			}}
		>
			<label>Name <input type="text" bind:value={nameDraft} /></label>
			<label>Description <input type="text" bind:value={descDraft} /></label>
			<label>Format <input type="text" bind:value={formatDraft} /></label>
			<div class="actions">
				<button type="submit" disabled={busy}>Save</button>
				<button type="button" class="plain" onclick={() => (renaming = false)}>Cancel</button>
			</div>
		</form>
	{/if}

	{#if deleting}
		<div class="confirm" role="alertdialog" aria-label="Delete group">
			<p>
				Delete "{detail.group.name}"? Choose what happens to its {count(detail.group.deck_count)}
				decks.
			</p>
			<div class="actions">
				<button onclick={() => removeGroup('ungroup')} disabled={busy}>
					Keep the decks, ungrouped
				</button>
				<button class="danger" onclick={() => removeGroup('delete')} disabled={busy}>
					Delete the decks too
				</button>
				<button class="plain" onclick={() => (deleting = false)}>Cancel</button>
			</div>
		</div>
	{/if}

	{#if detail.decks.length === 0}
		<EmptyState
			title="No decks in this group."
			description="Move a deck into the group from its own page, or name the group when creating a deck over MCP."
		/>
	{:else}
		<div class="grid">
			{#each detail.decks as deck (deck.id)}
				<a class="tile" href="/decks/{deck.id}">
					<div class="name">{deck.name}</div>
					<div class="meta">
						<span class="state state-{deck.state}">{deck.state}</span>
						{#if deck.format}· {deck.format}{/if}
					</div>
					<div class="count">{count(deck.card_count)} cards</div>
				</a>
			{/each}
		</div>
	{/if}
{/if}

<style>
	header {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		flex-wrap: wrap;
		gap: var(--space-4);
	}
	h1 {
		color: var(--color-text-accent);
		margin: 0;
		overflow-wrap: anywhere;
	}
	.crumb a {
		color: var(--color-text-subtle);
	}
	.sub,
	.desc {
		color: var(--color-text-subtle);
		font-size: var(--text-md);
		margin: var(--space-1) 0 0;
	}
	.muted {
		color: var(--color-text-subtle);
	}
	.error {
		color: var(--color-text-accent);
	}
	.actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.edit,
	.confirm {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: var(--color-surface-panel);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-xl);
		padding: var(--space-4);
		margin: var(--space-4) 0;
	}
	.edit label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		font-size: var(--text-md);
		color: var(--color-text-subtle);
	}
	input {
		padding: var(--space-2);
		min-width: 0;
		background: var(--color-surface-page);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		color: var(--color-text);
	}
	button {
		background: var(--color-accent);
		border: none;
		color: var(--color-on-accent);
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-md);
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.5;
	}
	button.danger {
		background: var(--color-danger);
	}
	button.plain {
		background: none;
		color: var(--color-text-subtle);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: var(--space-4);
		margin-top: var(--space-4);
	}
	.tile {
		display: block;
		background: var(--color-surface-panel);
		border: 2px solid var(--color-border);
		border-radius: var(--radius-xl);
		padding: var(--space-4);
		text-decoration: none;
		color: var(--color-text);
	}
	.tile:hover {
		border-color: var(--color-border-accent);
	}
	.name {
		font-weight: 700;
		color: var(--color-text-accent);
	}
	.meta {
		font-size: var(--text-sm);
		color: var(--color-text-subtle);
		margin: var(--space-1) 0;
	}
	.state {
		text-transform: uppercase;
		font-size: var(--text-xs);
		padding: var(--space-0-5) var(--space-2);
		border-radius: var(--radius-sm);
		background: var(--color-info-surface);
	}
	.state-built {
		background: var(--color-success-surface);
		color: var(--color-success-text);
	}
	.state-ready {
		background: var(--color-warning-surface);
		color: var(--color-warning-text);
	}
	.count {
		font-size: var(--text-md);
		color: var(--color-text-subtle);
	}
</style>
