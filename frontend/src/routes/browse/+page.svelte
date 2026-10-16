<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api } from '$lib/api';
	import { count } from '$lib/format';
	import { Button, EmptyState, Field, Panel, ProgressBar, Toolbar } from '$lib/components/ui';
	import type { SetSummary } from '$lib/types/SetSummary';
	import { groupSets, startsCollapsed, type SetGroup } from '$lib/setGroups';

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

	const groups = $derived(
		groupSets(visible).map((g) => ({
			...g,
			total_cards: g.sets.reduce((n, x) => n + x.total_cards, 0),
			owned_cards: g.sets.reduce((n, x) => n + x.owned_cards, 0)
		}))
	);

	// Category headers open and close individually.
	// A category the viewer has not toggled starts open only if they own a card in it.
	let collapsed = $state<Record<string, boolean>>({});
	function isCollapsed(g: { category: string; sets: SetSummary[] }): boolean {
		return collapsed[g.category] ?? startsCollapsed(g as SetGroup);
	}

	function pct(owned: number, total: number): number {
		return total > 0 ? Math.round((owned / total) * 100) : 0;
	}
	function anchorId(slug: string): string {
		return 'cat-' + slug;
	}
	function toggle(g: { category: string; sets: SetSummary[] }) {
		collapsed[g.category] = !isCollapsed(g);
	}
	function setAll(v: boolean) {
		const next: Record<string, boolean> = {};
		for (const g of groups) next[g.category] = v;
		collapsed = next;
	}
	/** Jump-to click: force-open the category and scroll to its header. */
	async function jumpTo(g: { category: string; slug: string }) {
		collapsed[g.category] = false;
		await tick();
		document
			.getElementById(anchorId(g.slug))
			?.scrollIntoView({ behavior: 'smooth', block: 'start' });
	}
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
			<span class="spacer"></span>
			<Button variant="ghost" size="sm" onclick={() => setAll(false)}>Expand all</Button>
			<Button variant="ghost" size="sm" onclick={() => setAll(true)}>Collapse all</Button>
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
				<aside class="navcol">
					<Panel class="nav" padding="sm">
						<div class="nav-label">Jump to</div>
						<ul>
							{#each groups as g (g.category)}
								<li>
									<button
										class="navlink"
										class:owned={g.owned_cards > 0}
										onclick={() => jumpTo(g)}
									>
										<span class="dot" aria-hidden="true"></span>
										<span class="name">{g.category}</span>
										<span class="setn">{g.sets.length}</span>
									</button>
								</li>
							{/each}
						</ul>
					</Panel>
				</aside>

				<div class="main">
					{#each groups as g (g.category)}
						<section class="seriesgroup" id={anchorId(g.slug)}>
							<button
								class="grouphdr"
								onclick={() => toggle(g)}
								aria-expanded={!isCollapsed(g)}
							>
								<span class="caret" aria-hidden="true">{isCollapsed(g) ? '▸' : '▾'}</span>
								<span class="ghname">{g.category}</span>
								<span class="ghmeta">
									{g.sets.length}
									{g.sets.length === 1 ? 'set' : 'sets'}
									{#if g.total_cards > 0}
										· {count(g.owned_cards)} / {count(g.total_cards)} cards ({pct(
											g.owned_cards,
											g.total_cards
										)}%)
									{/if}
								</span>
							</button>
							{#if !isCollapsed(g)}
								<div class="grid">
									{#each g.sets as set (set.set_code)}
										<Panel class="tile" href="/browse/{set.set_code}">
											{#if set.art_url}
												<img class="crop" src={set.art_url} alt="" loading="lazy" />
											{:else}
												<div class="crop placeholder" aria-hidden="true"></div>
											{/if}
											<div class="ident">
												<span class="symbol">
													{#if set.logo_url}
														<img class="logo" src={set.logo_url} alt="{set.name} logo" loading="lazy" />
													{:else}
														<span class="code">{set.set_code.toUpperCase()}</span>
													{/if}
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
							{/if}
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
	.spacer {
		flex: 1;
	}
	/* Sticky Jump-to column beside the groups; on narrow viewports the list
	   becomes a horizontal wrapping chip row above them (never a sideways scroll). */
	.layout {
		display: grid;
		grid-template-columns: 200px minmax(0, 1fr);
		gap: var(--space-5);
		align-items: start;
		margin-top: var(--space-4);
	}
	.navcol {
		position: sticky;
		top: var(--space-2);
	}
	.navcol :global(.nav) {
		max-height: calc(100vh - var(--space-4));
		overflow-y: auto;
	}
	.nav-label {
		text-transform: uppercase;
		font-size: var(--text-xs);
		letter-spacing: 0.06em;
		color: var(--color-text-subtle);
		padding: var(--space-1) var(--space-1) var(--space-2);
	}
	.navcol ul {
		list-style: none;
		padding: var(--space-0);
		margin: var(--space-0);
		display: flex;
		flex-direction: column;
		gap: var(--space-px);
	}
	.navlink {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		width: 100%;
		background: none;
		border: none;
		color: var(--color-text-subtle);
		padding: var(--space-1);
		border-radius: var(--radius-sm);
		cursor: pointer;
		font: inherit;
		font-size: var(--text-md);
		text-align: left;
	}
	.navlink:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-strong);
	}
	.navlink:focus-visible {
		outline: none;
		box-shadow: var(--shadow-focus);
	}
	.navlink.owned {
		color: var(--color-text);
	}
	.dot {
		width: var(--space-2);
		height: var(--space-2);
		border-radius: var(--radius-round);
		background: var(--color-neutral);
		flex-shrink: 0;
	}
	.navlink.owned .dot {
		background: var(--color-success);
	}
	.navlink .name {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.navlink .setn {
		color: var(--color-text-subtle);
		font-variant-numeric: tabular-nums;
		font-size: var(--text-sm);
	}
	.main {
		min-width: 0;
	}
	.seriesgroup {
		margin-bottom: var(--space-6);
	}
	.grouphdr {
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		width: 100%;
		background: none;
		border: none;
		border-bottom: 1px solid var(--color-border);
		border-radius: var(--radius-sm) var(--radius-sm) 0 0;
		color: var(--color-text);
		padding: var(--space-2);
		font: inherit;
		font-size: var(--text-lg);
		cursor: pointer;
		text-align: left;
	}
	.grouphdr:hover {
		background: var(--color-surface-hover);
	}
	.grouphdr:focus-visible {
		outline: none;
		box-shadow: var(--shadow-focus);
	}
	.caret {
		width: 1ch;
		color: var(--color-text-decorative);
		font-size: var(--text-sm);
	}
	.ghname {
		font-weight: var(--weight-bold);
		color: var(--color-text-strong);
	}
	.ghmeta {
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
		font-variant-numeric: tabular-nums;
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
	.symbol .logo {
		max-width: 100%;
		max-height: 100%;
		object-fit: contain;
	}
	/* The set's art crop — the illustration without the card frame — as a
	   banner across the top of the tile, served from /art, never the CDN. */
	.crop {
		display: block;
		width: 100%;
		height: var(--space-16, 72px);
		object-fit: cover;
		border-radius: var(--radius-sm);
	}
	/* A set with no retrieved crop keeps the banner's footprint, so tiles
	   stay one height. */
	.crop.placeholder {
		background: var(--color-surface-sunken);
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

	@media (max-width: 720px) {
		/* `minmax(0, 1fr)`, not `1fr`: a bare 1fr track has an automatic
		   minimum of its content's min-content, and the chip row's
		   chips add up to wider than a phone, so the track grew to fit them
		   and the tile grid below fitted 7 columns into it. */
		.layout {
			grid-template-columns: minmax(0, 1fr);
		}
		.navcol {
			position: static;
		}
		.navcol :global(.nav) {
			max-height: none;
		}
		.navcol ul {
			flex-direction: row;
			flex-wrap: wrap;
			gap: var(--space-1);
		}
		.navlink {
			flex: 0 1 auto;
			width: auto;
			max-width: 100%;
			padding: var(--space-1) var(--space-2);
			border: 1px solid var(--color-border);
			border-radius: var(--radius-pill);
		}
		.navlink .setn {
			display: none;
		}
	}
</style>
