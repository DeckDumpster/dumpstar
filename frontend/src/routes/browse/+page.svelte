<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import { count } from '$lib/format';
	import { Button, EmptyState, Field, Panel, ProgressBar, Toolbar } from '$lib/components/ui';
	import type { SetSummary } from '$lib/types/SetSummary';
	import { groupSets } from '$lib/setGroups';

	let sets = $state<SetSummary[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let search = $state('');

	onMount(async () => {
		try {
			sets = await api.sets();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	});

	/** The API answers newest release first; search narrows by name or code. */
	const visible = $derived.by(() => {
		const q = search.trim().toLowerCase();
		return sets.filter(
			(c) => !q || c.name.toLowerCase().includes(q) || c.set_code.toLowerCase().includes(q)
		);
	});

	const groups = $derived(groupSets(visible));
</script>

<svelte:head><title>Browse sets — Dumpstar</title></svelte:head>

<header class="page">
	<div>
		<h1>Browse sets</h1>
		<p class="muted">Pick a set to open its binder view.</p>
	</div>
</header>

{#if loading}
	<p class="muted">Loading…</p>
{:else if error}
	<p class="error">Failed to load sets: {error}</p>
{:else}
	<div class="browsepage">
		<Toolbar class="controls" gap="sm">
			<Field
				class="search"
				type="text"
				placeholder="Search sets by name or code…"
				aria-label="Search sets by name or code"
				bind:value={search}
			/>
			<span class="counts">{visible.length} of {sets.length}</span>
		</Toolbar>

		{#if visible.length === 0}
			{#if search.trim()}
				<EmptyState
					title="No sets match “{search.trim()}”."
					description="The search reads set names and codes — try a shorter fragment."
				>
					{#snippet action()}
						<Button variant="ghost" onclick={() => (search = '')}>Clear search</Button>
					{/snippet}
				</EmptyState>
			{:else}
				<EmptyState
					title="No sets in the catalog."
					description="The shared catalog hasn't been built yet — until it is, there are no binder pages to browse."
				/>
			{/if}
		{:else}
			<div class="layout">
				<nav class="categories" aria-label="Set categories">
					{#each groups as g (g.category)}
						<a href="#cat-{g.slug}"
							>{g.category} <span class="n">{g.sets.length}</span></a
						>
					{/each}
				</nav>
				<div class="groups">
					{#each groups as g (g.category)}
						<section id="cat-{g.slug}">
							<h2>{g.category}</h2>
							<div class="grid">
								{#each g.sets as set (set.set_code)}
					<Panel class="tile" href="/browse/{set.set_code}">
						<div class="ident">
							<span class="symbol">
								<span class="code">{set.set_code.toUpperCase()}</span>
							</span>
							<span class="names">
								<span class="title">{set.name}</span>
								{#if set.release_date}
									<span class="series">{set.release_date}</span>
								{/if}
							</span>
						</div>
						<div class="meters">
							<ProgressBar
								tone="complete"
								label="Cards {count(set.owned_cards)} / {count(set.total_cards)}"
								value={set.owned_cards}
								max={set.total_cards}
							/>
							<ProgressBar
								tone="complete"
								label="Master {count(set.owned_printings)} / {count(set.total_printings)}"
								value={set.owned_printings}
								max={set.total_printings}
							/>
						</div>
					</Panel>
								{/each}
							</div>
						</section>
					{/each}
				</div>
			</div>
		{/if}
	</div>
{/if}

<style>
	/*
		ONE MEANING PER COLOUR: crimson (accent) is the app's own voice — the
		page title and a tile's hover edge, never data; green (progress) is
		ownership and completion, the only hue that encodes a number; neutral
		(ink/slate) is structure and content.

		Only layout and geometry are left in this block. Surfaces, fills,
		rules, text colour, radius, spacing and elevation arrive through the
		semantic token layer or through a primitive that owns them.

		WHERE A PRIMITIVE IS PLACED. Svelte scopes a rule to the elements this
		file declares, so a class handed to a component compiles to a selector
		matching nothing. Placement is written as `:global()` nested under a
		scoped ancestor — `.browsepage` exists to be that ancestor for the
		page's top-level rows. Never a bare `:global()`, which would leak the
		rule to every route.
	*/
	h1 {
		color: var(--color-text-accent);
		margin-bottom: var(--space-1);
	}
	.page {
		display: flex;
		gap: var(--space-4);
		align-items: baseline;
		justify-content: space-between;
	}
	.muted {
		color: var(--color-text-subtle);
	}
	.error {
		color: var(--color-danger-text);
	}
	.browsepage :global(.controls) {
		margin: var(--space-3) var(--space-0);
	}
	.browsepage :global(.search) {
		flex: 1;
		min-width: 220px;
		max-width: 360px;
	}
	.counts {
		color: var(--color-text-subtle);
		font-size: var(--text-md);
	}

	/* GUTTER RHYTHM AND THE RAGGED LAST ROW. Columns are one track repeated,
	   so a tile is the same width in every row of every group — a short final
	   row stops, it does not stretch to fill. `grid-auto-rows: 1fr` gives
	   every row the same height, and the meters block is pinned to the bottom
	   of its tile, so the last bar lands on one line across the whole group
	   whether a set shows one meter or two. Rows are gapped one step wider
	   than columns: the eye reads a row as a row. */
	.layout {
		display: flex;
		gap: var(--space-6);
		align-items: flex-start;
		margin-top: var(--space-4);
	}
	.categories {
		position: sticky;
		top: var(--space-4);
		flex: 0 0 220px;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.categories a {
		display: flex;
		justify-content: space-between;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-sm);
		color: var(--color-text-strong);
		text-decoration: none;
	}
	.categories a:hover {
		background: var(--color-surface-inset);
	}
	.categories .n {
		color: var(--color-text-subtle);
	}
	.groups {
		flex: 1;
		min-width: 0;
	}
	.groups h2 {
		margin-top: var(--space-5);
	}
	.groups section:first-child h2 {
		margin-top: 0;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		grid-auto-rows: 1fr;
		column-gap: var(--space-4);
		row-gap: var(--space-5);
		margin-top: var(--space-4);
	}
	/* No `height: 100%` here: a grid item already stretches to its row, and
	   the app sets no global `box-sizing`, so a percentage height would add
	   the tile's padding on top of the row and overflow the group. */
	.grid :global(.tile) {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.ident {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		min-width: 0;
	}
	/* Upstream ships the set symbol as a white-on-near-black raster of the
	   set's code — SSP, MEW, TTBB-2024 — at whatever aspect it likes. Dropped
	   straight onto the panel it read as a chip pasted in from another app.
	   The well is the fix: the darkest surface in the ramp, so the raster's
	   own ground melts into it, one square size whatever the art's aspect, and
	   a rule that is the same rule every other box on this page draws. */
	.symbol {
		box-sizing: border-box;
		display: grid;
		place-items: center;
		flex: 0 0 auto;
		width: var(--space-10);
		height: var(--space-10);
		padding: var(--space-1);
		background: var(--color-surface-inset);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		overflow: hidden;
	}
	.symbol .code {
		font-size: var(--text-xs);
		font-weight: var(--weight-semibold);
		color: var(--color-text-subtle);
		letter-spacing: 0.04em;
	}
	.names {
		display: flex;
		flex-direction: column;
		gap: var(--space-0-5);
		min-width: 0;
	}
	/* Two lines, then an ellipsis — a long set name may not decide how tall
	   every tile in the group is. */
	.title {
		font-weight: var(--weight-semibold);
		color: var(--color-text-strong);
		line-height: var(--leading-tight);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.series {
		font-size: var(--text-sm);
		color: var(--color-text-subtle);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.meters {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-top: auto;
	}
</style>
