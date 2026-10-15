<script lang="ts">
	import { fade } from 'svelte/transition';
	import { api } from '$lib/api';
	import { facetHref } from '$lib/facets';
	import { sortAspects } from '$lib/aspects';
	import AspectIcon from '$lib/components/AspectIcon.svelte';
	import { breadcrumbs } from '$lib/breadcrumbs.svelte';
	import { variantSortCmp } from '$lib/variants.svelte';
	import { CONDITIONS } from '$lib/conditions';
	import { conditionMultiplier } from '$lib/conditions.svelte';
	import { money as price, printingLabel, jsonList } from '$lib/format';
	import type { CardDetail } from '$lib/types/CardDetail';
	import type { Binder } from '$lib/types/Binder';
	import type { Deck } from '$lib/types/Deck';
	import type { PriceSeries } from '$lib/types/PriceSeries';
	import PriceChart from './PriceChart.svelte';
	import ManualPriceModal from './ManualPriceModal.svelte';
	import MissingVariantModal from './MissingVariantModal.svelte';
	import { Badge, Button, EmptyState, SectionHeader } from '$lib/components/ui';

	// The card-detail body, shared by the /card/[set]/[number] route and the
	// collection-page modal. Self-contained: it fetches its own data.
	let {
		setCode,
		number,
		onMutate,
		manageBreadcrumbs = false
	}: {
		setCode: string;
		number: string;
		/** Fired after any successful copy mutation (add/remove/status/
		 *  condition/variant/assign). The collection modal uses this to
		 *  decide whether closing needs a re-fetch — viewing a card without
		 *  touching it leaves the list untouched, so no reload. */
		onMutate?: () => void;
		/** When true, override the layout breadcrumb trail to "Browse ›
		 *  <set name> › <card name> #<number>". The /card route page
		 *  sets this; the CardModal wrapper (used inside /collection) does
		 *  not, so the modal doesn't clobber the host page's crumbs. */
		manageBreadcrumbs?: boolean;
	} = $props();

	let detail = $state<CardDetail | null>(null);
	let binders = $state<Binder[]>([]);
	let decks = $state<Deck[]>([]);
	let priceSeries = $state<PriceSeries[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	// Per-control in-flight keys (e.g. "123:condition", "add:sor-5-foil").
	// Only the control being saved is disabled, so edits to different copies
	// or fields run concurrently instead of serializing behind one another's
	// save + reload (dumpstar-gxug).
	let savingKeys = $state<Set<string>>(new Set());
	const isSaving = (key: string) => savingKeys.has(key);
	let priceModalFor = $state<{ printing_id: string; label: string } | null>(null);
	let missingVariantOpen = $state(false);

	const STATUSES = ['owned', 'ordered', 'listed', 'sold', 'removed', 'traded', 'gifted', 'lost'];

	async function load() {
		if (!setCode || !number) return;
		loading = true;
		error = null;
		try {
			[detail, binders, decks, priceSeries] = await Promise.all([
				api.card(setCode, number),
				api.binders(),
				api.decks(),
				api.cardPrices(setCode, number)
			]);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void setCode;
		void number;
		load();
	});

	// Upgrade the breadcrumb labels once card data resolves — the route
	// page seeded a synchronous placeholder using URL params, this swaps
	// the bare set_code for the set's name and "#4" for "Luke Skywalker #4". $effect.pre
	// runs before the DOM updates so the upgrade lands in the same frame
	// as the data; no visible placeholder-to-label flash.
	// No unmount cleanup needed — the breadcrumbs store is path-keyed
	// and auto-discards overrides when the URL changes.
	$effect.pre(() => {
		if (!manageBreadcrumbs) return;
		const card = detail?.card;
		if (!card) return;
		breadcrumbs.set([
			{ label: 'Browse', href: '/browse' },
			{ label: card.set_name, href: `/browse/${card.set_code}` },
			{ label: `${card.name} #${card.number}` }
		]);
	});

	// Inline per-control save confirmation: after a successful edit, flash a ✓
	// next to the exact control that changed (dumpstar-25r). The key is
	// `${copyId}:${field}`.
	let savedKey = $state<string | null>(null);
	let savedTimer: ReturnType<typeof setTimeout> | undefined;
	function flashSaved(key: string | undefined) {
		if (!key) return;
		savedKey = key;
		clearTimeout(savedTimer);
		savedTimer = setTimeout(() => (savedKey = null), 1600);
	}

	// `key` identifies the control being saved ("<copyId>:<field>" or
	// "add:/remove:<printingId>"). It scopes the disabled state (so only that
	// control locks while it saves) and the inline ✓. Concurrent edits to
	// other controls run in parallel; each does its own reload and converges.
	async function withBusy(fn: () => Promise<unknown>, key: string) {
		savingKeys = new Set(savingKeys).add(key);
		error = null;
		try {
			await fn();
			onMutate?.();
			await load();
			flashSaved(key);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			const next = new Set(savingKeys);
			next.delete(key);
			savingKeys = next;
		}
	}

	const addCopy = (printingId: string) =>
		withBusy(() => api.addCopy({ printing_id: printingId, source: 'manual' }), `add:${printingId}`);
	const removeCopy = (printingId: string) =>
		withBusy(() => api.removeCopyByPrinting(printingId), `remove:${printingId}`);
	const changeVariant = (copyId: number, printingId: string) =>
		withBusy(() => api.changePrinting(copyId, printingId), `${copyId}:variant`);
	const changeStatus = (copyId: number, status: string) =>
		withBusy(() => api.setCopyStatus(copyId, status), `${copyId}:status`);
	const changeCondition = (copyId: number, condition: string) =>
		withBusy(() => api.updateCopy(copyId, { condition }), `${copyId}:condition`);
	const changeNotes = (copyId: number, notes: string) =>
		withBusy(() => api.updateCopy(copyId, { notes }), `${copyId}:notes`);

	function assignValue(copy: { binder_id: number | null; deck_id: number | null }): string {
		if (copy.binder_id != null) return `b:${copy.binder_id}`;
		if (copy.deck_id != null) return `d:${copy.deck_id}`;
		return '';
	}
	/** "b:3" → assign binder 3, "d:5" → assign deck 5, "" → unassign. */
	function assignBody(value: string): { binder_id?: number; deck_id?: number } {
		if (value.startsWith('b:')) return { binder_id: Number(value.slice(2)) };
		if (value.startsWith('d:')) return { deck_id: Number(value.slice(2)) };
		return {};
	}
	function assignCopy(copyId: number, value: string) {
		return withBusy(() => api.moveCopy(copyId, assignBody(value)), `${copyId}:location`);
	}

	// --- Multi-select copies for bulk edit. The user often grades a stack of
	//     the same card and wants to set condition/status/location across many
	//     copies at once instead of row by row (dumpstar-0qu). Only shown
	//     when the card has more than one owned copy. ---
	let selectedCopies = $state<Set<number>>(new Set());
	const copyIds = $derived((detail?.copies ?? []).map((c) => c.id));
	const multiCopy = $derived(copyIds.length > 1);
	const allCopiesChecked = $derived(
		copyIds.length > 0 && copyIds.every((id) => selectedCopies.has(id))
	);
	// Drop any selection that no longer maps to a live copy (e.g. after a
	// reload removed one), so a bulk action never targets a stale id.
	$effect(() => {
		const live = new Set(copyIds);
		if ([...selectedCopies].some((id) => !live.has(id))) {
			selectedCopies = new Set([...selectedCopies].filter((id) => live.has(id)));
		}
	});
	function toggleCopy(id: number) {
		const next = new Set(selectedCopies);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		selectedCopies = next;
	}
	function toggleAllCopies() {
		selectedCopies = allCopiesChecked ? new Set() : new Set(copyIds);
	}

	/** Apply an edit to every selected copy at once, then reload. */
	async function bulkCopies(fn: (id: number) => Promise<unknown>) {
		const live = new Set(copyIds);
		const ids = [...selectedCopies].filter((id) => live.has(id));
		if (!ids.length) return;
		savingKeys = new Set(savingKeys).add('bulk');
		error = null;
		try {
			await Promise.all(ids.map(fn));
			onMutate?.();
			await load();
			flashSaved('bulk');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			const next = new Set(savingKeys);
			next.delete('bulk');
			savingKeys = next;
		}
	}
	const bulkCondition = (c: string) => {
		if (c) void bulkCopies((id) => api.updateCopy(id, { condition: c }));
	};
	const bulkStatus = (s: string) => {
		if (s) void bulkCopies((id) => api.setCopyStatus(id, s));
	};
	const bulkLocation = (value: string) => {
		if (value !== '__none__') void bulkCopies((id) => api.moveCopy(id, assignBody(value)));
	};

	/** Per-copy estimated value: the copy's printing market price scaled
	 *  by the copy's condition multiplier. Null when we don't have a
	 *  market price for the printing (e.g. user_printings entries that
	 *  haven't had a manual price set). */
	function copyValue(copy: { printing_id: string; condition: string }): number | null {
		const p = detail?.printings.find((x) => x.printing_id === copy.printing_id);
		if (!p || p.market_price == null) return null;
		return p.market_price * conditionMultiplier(copy.condition);
	}

	// Price chart: by default only plot the variants the user owns, so an
	// unowned chase printing (e.g. a $500 Showcase Leader NM) doesn't dwarf the
	// lines that matter (dumpstar-vgo). A toggle reveals the rest; if the
	// user owns no priced variant, fall back to showing all so the chart isn't
	// empty.
	let showAllPrices = $state(false);
	// Every printing of the card, ordered by variant then finish (Normal
	// before Foil). The add-list, the per-copy Variant dropdown and the price
	// chart all key off this.
	const visiblePrintings = $derived(
		(detail?.printings ?? [])
			.slice()
			.sort(
				(a, b) =>
					variantSortCmp(a.variant, b.variant) ||
					Number(a.finish === 'Foil') - Number(b.finish === 'Foil') ||
					a.set_code.localeCompare(b.set_code)
			)
	);
	const visiblePrintingIds = $derived(new Set(visiblePrintings.map((p) => p.printing_id)));
	const ownedPrintingIds = $derived(
		new Set((detail?.printings ?? []).filter((p) => p.owned_count > 0).map((p) => p.printing_id))
	);
	const allSeries = $derived(priceSeries.filter((s) => visiblePrintingIds.has(s.printing_id)));
	const ownedSeries = $derived(priceSeries.filter((s) => ownedPrintingIds.has(s.printing_id)));
	const chartSeries = $derived(showAllPrices || ownedSeries.length === 0 ? allSeries : ownedSeries);
	// Show the toggle only when there's something hidden to reveal.
	const hasHiddenSeries = $derived(ownedSeries.length > 0 && allSeries.length > ownedSeries.length);

	/** A printing as one line: `SOR #5 · Hyperspace Foil`. */
	function printingTitle(p: { set_code: string; number: string; variant: string; finish: string }) {
		return `${p.set_code.toUpperCase()} #${p.number} · ${printingLabel(p.variant, p.finish)}`;
	}
</script>

<svelte:head>
	<title>{detail ? detail.card.name : 'Card'} — Dumpstar</title>
</svelte:head>

{#if loading && !detail}
	<p class="muted">Loading…</p>
{:else if error && !detail}
	<p class="error">Failed to load card: {error}</p>
{:else if detail}
	{@const card = detail.card}

	<div class="detail">
		<div class="art">
			{#if card.image_url}
				<img src={card.image_url} alt={card.name} />
			{:else}
				<div class="noart">No image</div>
			{/if}
		</div>
		<div class="info">
			<h1>{card.name}</h1>
			{#if card.subtitle}<p class="subtitle">{card.subtitle}</p>{/if}
			<p class="sub">
				<a
					class="facet"
					href={facetHref('set', card.set_code)}
					title="Find all cards in {card.set_name}"
				>
					<span>{card.set_code.toUpperCase()}</span>
				</a>
				· #{card.number}{#if card.rarity}
					·
					<a class="facet" href={facetHref('rarity', card.rarity)} title="Find all {card.rarity} cards">
						{card.rarity}
					</a>{/if}
				· <a class="facet binderlink" href="/browse/{card.set_code}" title="Open {card.set_name} binder">{card.set_name}</a>
			</p>
			<dl>
				{#if card.card_type}<dt>Type</dt><dd>
						<a class="facet" href={facetHref('type', card.card_type)}>{card.card_type}</a>
					</dd>{/if}
				{#if card.cost != null}<dt>Cost</dt><dd>{card.cost}</dd>{/if}
				{#if card.power != null}<dt>Power</dt><dd>{card.power}</dd>{/if}
				{#if card.hp != null}<dt>HP</dt><dd>{card.hp}</dd>{/if}
				{#if jsonList(card.aspects).length}
					<dt>Aspects</dt>
					<dd>
						{#each sortAspects(jsonList(card.aspects)) as a (a)}
							<a class="facet chip" href={facetHref('aspect', a)} title="Find all {a} cards"><AspectIcon name={a} /></a>
						{/each}
					</dd>
				{/if}
				{#if jsonList(card.traits).length}
					<dt>Traits</dt>
					<dd>
						{#each jsonList(card.traits) as t (t)}
							<a class="facet chip" href={facetHref('trait', t)} title="Find all {t} cards">{t}</a>
						{/each}
					</dd>
				{/if}
				{#if jsonList(card.arenas).length}
					<dt>Arena</dt>
					<dd>
						{#each jsonList(card.arenas) as a (a)}
							<a class="facet chip" href={facetHref('arena', a)} title="Find all {a} cards">{a}</a>
						{/each}
					</dd>
				{/if}
				{#if card.artist}<dt>Artist</dt><dd>
						<a class="facet" href={facetHref('artist', card.artist)} title="Find all cards by {card.artist}">{card.artist}</a>
					</dd>{/if}
			</dl>

			{#if card.rules_text}<p class="cardText">{card.rules_text}</p>{/if}
		</div>
	</div>

	{#if error}<p class="error">{error}</p>{/if}

	<section>
		<SectionHeader title="Printings" size="md" tone="accent">
			{#snippet actions()}
				<Button
					variant="ghost"
					size="sm"
					onclick={() => (missingVariantOpen = true)}
					title="Add a copy whose variant isn't yet in the catalog">+ Missing variant</Button
				>
			{/snippet}
		</SectionHeader>
		<ul class="printings">
			{#each visiblePrintings as p (p.printing_id)}
				<li class:user-added={p.is_user_added}>
					<div class="vlabel">
						<div class="vline">
							{#if p.is_user_added}
								<span
									class="user-pip"
									title="User-added via the missing-variant escape hatch"
									aria-label="User-added variant"
								></span>
							{/if}
							<a class="facet variant" href={facetHref('variant', p.variant)} title="Find all {p.variant} printings">
								{printingTitle(p)}
							</a>
						</div>
						{#if p.is_user_added && p.description}
							<span class="provenance">{p.description}</span>
						{/if}
					</div>
					<span class="market">{price(p.market_price)}</span>
					{#if p.tcgplayer_product_id != null}
						<a
							class="tcgp"
							href="https://www.tcgplayer.com/product/{p.tcgplayer_product_id}"
							target="_blank"
							rel="noopener"
							title="Open on TCGplayer"
						>TCG↗</a>
					{:else}
						<span class="tcgp-spacer"></span>
					{/if}
					<!-- Only a printing this user invented can carry a manual price.
					     The price of a catalog printing is the catalog's, the same for
					     every tenant — the API refuses the write with a 400, so
					     offering the button here would be an affordance that cannot
					     succeed (pd-m4gw). -->
					{#if p.is_user_added}
						<button
							class="manual-price"
							onclick={() => (priceModalFor = {
								printing_id: p.printing_id,
								label: `${card.name} — ${printingTitle(p)}`
							})}
							title="Record a manual price"
							aria-label="Record a manual price for {printingTitle(p)}"
						>$</button>
					{:else}
						<span class="manual-price-spacer"></span>
					{/if}
					<div class="stepper">
						<button
							class="step"
							disabled={isSaving(`remove:${p.printing_id}`) || p.owned_count <= 0}
							onclick={() => removeCopy(p.printing_id)}
							aria-label="Remove one {printingTitle(p)}"
						>−</button>
						<span class="count" class:has={p.owned_count > 0}>{p.owned_count}</span>
						<button
							class="step"
							disabled={isSaving(`add:${p.printing_id}`)}
							onclick={() => addCopy(p.printing_id)}
							aria-label="Add one {printingTitle(p)}"
						>+</button>
					</div>
				</li>
			{/each}
		</ul>
	</section>

	<section>
		<SectionHeader title="Price history" size="md" tone="accent">
			{#snippet actions()}
				{#if hasHiddenSeries}
					<label class="showall">
						<input type="checkbox" bind:checked={showAllPrices} />
						Show all printings
					</label>
				{/if}
			{/snippet}
		</SectionHeader>
		<PriceChart series={chartSeries} />
	</section>

	<section>
		<SectionHeader title="Your copies ({detail.copies.length})" size="md" tone="accent" />
		{#if detail.copies.length === 0}
			<EmptyState
				size="sm"
				title="You don't own this card yet."
				description="Register a printing above and each copy you own lands here as its own row, with its own condition and location."
			/>
		{:else}
			{#snippet savedTick(key: string)}
				{#if savedKey === key}<span class="cellSaved" transition:fade={{ duration: 120 }}
						>✓</span
					>{/if}
			{/snippet}
			{#if multiCopy && selectedCopies.size > 0}
				<div class="copybulk">
					<span class="count">{selectedCopies.size} selected</span>
					<select
						data-testid="bulk-condition"
						disabled={isSaving('bulk')}
						onchange={(e) => {
							bulkCondition(e.currentTarget.value);
							e.currentTarget.selectedIndex = 0;
						}}
					>
						<option value="">Set condition…</option>
						{#each CONDITIONS as c (c)}<option value={c}>{c}</option>{/each}
					</select>
					<select
						disabled={isSaving('bulk')}
						onchange={(e) => {
							bulkStatus(e.currentTarget.value);
							e.currentTarget.selectedIndex = 0;
						}}
					>
						<option value="">Set status…</option>
						{#each STATUSES as s (s)}<option value={s}>{s}</option>{/each}
					</select>
					<select
						disabled={isSaving('bulk')}
						onchange={(e) => {
							bulkLocation(e.currentTarget.value);
							e.currentTarget.selectedIndex = 0;
						}}
					>
						<option value="__none__">Assign to…</option>
						<option value="">Unassigned</option>
						{#each binders as b (b.id)}<option value="b:{b.id}">Binder: {b.name}</option>{/each}
						{#each decks as d (d.id)}<option value="d:{d.id}">Deck: {d.name}</option>{/each}
					</select>
					<span class="bulkclear">
						<Button variant="ghost" size="sm" onclick={() => (selectedCopies = new Set())}
							>Clear</Button
						>
					</span>
				</div>
			{/if}
			<table>
				<colgroup>
					{#if multiCopy}<col style="width: 4%" />{/if}
					<col style="width: 19%" />
					<col style="width: 19%" />
					<col style="width: 15%" />
					<col style="width: 21%" />
					<col style="width: 13%" />
					<col style="width: 13%" />
				</colgroup>
				<thead>
					<tr>
						{#if multiCopy}<th class="selcol"
								><input
									type="checkbox"
									checked={allCopiesChecked}
									onchange={toggleAllCopies}
									title="Select all copies"
									aria-label="Select all copies"
								/></th
							>{/if}
						<th>Variant</th><th>Condition</th><th>Status</th><th>Location</th><th>Paid</th><th
							>Value</th
						>
					</tr>
				</thead>
				<tbody>
					{#each detail.copies as copy (copy.id)}
						<tr class="copyrow" class:picked={selectedCopies.has(copy.id)}>
							{#if multiCopy}<td class="selcol"
									><input
										type="checkbox"
										checked={selectedCopies.has(copy.id)}
										onchange={() => toggleCopy(copy.id)}
										aria-label="Select copy"
									/></td
								>{/if}
							<td data-label="Variant">
								<select
									value={copy.printing_id}
									disabled={isSaving(`${copy.id}:variant`)}
									onchange={(e) => changeVariant(copy.id, e.currentTarget.value)}
								>
									{#each visiblePrintings as p (p.printing_id)}
										<option value={p.printing_id}>{printingTitle(p)}</option>
									{/each}
								</select>
								{@render savedTick(`${copy.id}:variant`)}
							</td>
							<td data-label="Condition">
							<select
								value={copy.condition}
								disabled={isSaving(`${copy.id}:condition`)}
								onchange={(e) => changeCondition(copy.id, e.currentTarget.value)}
							>
								{#each CONDITIONS as c (c)}<option value={c}>{c}</option>{/each}
							</select>
							{@render savedTick(`${copy.id}:condition`)}
						</td>
							<td data-label="Status">
								<select
									value={copy.status}
									disabled={isSaving(`${copy.id}:status`)}
									onchange={(e) => changeStatus(copy.id, e.currentTarget.value)}
								>
									{#each STATUSES as s (s)}<option value={s}>{s}</option>{/each}
								</select>
								{@render savedTick(`${copy.id}:status`)}
							</td>
							<td data-label="Location">
								<select
									value={assignValue(copy)}
									disabled={isSaving(`${copy.id}:location`)}
									onchange={(e) => assignCopy(copy.id, e.currentTarget.value)}
								>
									<option value="">Unassigned</option>
									{#each binders as b (b.id)}<option value="b:{b.id}">Binder: {b.name}</option>{/each}
									{#each decks as d (d.id)}<option value="d:{d.id}">Deck: {d.name}</option>{/each}
								</select>
								{@render savedTick(`${copy.id}:location`)}
							</td>
							<td data-label="Paid">{price(copy.purchase_price)}</td>
							<td data-label="Value" title="NM market × condition multiplier"
								>{price(copyValue(copy))}</td
							>
						</tr>
						<tr class="noterow">
							<td colspan={multiCopy ? 7 : 6}>
								<input
									class="noteinput"
									type="text"
									value={copy.notes ?? ''}
									disabled={isSaving(`${copy.id}:notes`)}
									placeholder="Notes — e.g. creased corner, two visible bumps"
									title="Condition notes for this copy"
									onchange={(e) => changeNotes(copy.id, e.currentTarget.value)}
								/>
								{@render savedTick(`${copy.id}:notes`)}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
{/if}

{#if priceModalFor}
	<ManualPriceModal
		printingId={priceModalFor.printing_id}
		label={priceModalFor.label}
		onClose={() => (priceModalFor = null)}
		onChange={() => api.cardPrices(setCode, number).then((s) => (priceSeries = s))}
	/>
{/if}

{#if missingVariantOpen && detail}
	<MissingVariantModal
		cardId={detail.card.card_id}
		cardLabel="{detail.card.name} #{detail.card.number}"
		onClose={() => (missingVariantOpen = false)}
		onCreated={() => {
			missingVariantOpen = false;
			load();
		}}
	/>
{/if}

<style>
	.muted {
		color: var(--color-text-subtle);
	}
	.error {
		color: var(--color-danger-text);
	}
	/* Inline per-control save confirmation: a small green ✓ that flashes in
	   the top-right corner of the control just edited (dumpstar-25r). */
	.cellSaved {
		position: absolute;
		top: 2px;
		right: 2px;
		width: 15px;
		height: 15px;
		border-radius: var(--radius-round);
		background: var(--color-success);
		color: var(--color-on-success);
		font-size: 0.62rem;
		line-height: 1;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		/* Ring so it reads as a badge sitting over the select's corner. */
		box-shadow: 0 0 0 2px var(--color-surface-page);
		pointer-events: none;
	}
	.detail {
		display: flex;
		gap: var(--space-6);
		flex-wrap: wrap;
		/* Centers each flex row, so when .info wraps below .art the lone
		   card image sits in the middle of the viewport (mirrors how DD
		   centers its card-image-section). */
		justify-content: center;
	}
	.art img {
		width: 320px;
		max-width: 80vw;
		border-radius: var(--radius-xl);
	}
	.noart {
		width: 320px;
		height: 446px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-surface-panel);
		border-radius: var(--radius-xl);
		color: var(--color-text-subtle);
	}
	.info {
		flex: 1;
		min-width: 260px;
	}
	h1 {
		color: var(--color-text-accent);
		margin: var(--space-0);
	}
	.sub {
		color: var(--color-text-subtle);
		margin: var(--space-1) var(--space-0) var(--space-4);
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		flex-wrap: wrap;
	}
	dl {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 0.3rem 1rem;
		margin: var(--space-0);
	}
	dt {
		color: var(--color-text-subtle);
		font-size: var(--text-md);
	}
	dd {
		margin: var(--space-0);
	}
	/* Clickable facet — set glyph, rarity, type, artist, variant. Looks
	   like body text by default; hover hints the link affordance. */
	.facet {
		color: inherit;
		text-decoration: none;
		cursor: pointer;
		border-radius: var(--radius-sm);
		padding: 1px 3px;
	}
	.chip {
		display: inline-block;
		margin-right: var(--space-1);
		border: 1px solid var(--color-border);
	}
	.subtitle {
		margin: var(--space-0) var(--space-0) var(--space-1);
		color: var(--color-text-subtle);
	}
	.facet:hover {
		background: var(--color-surface-selected);
		color: var(--color-text);
	}
		section {
		margin-top: var(--space-8);
	}

	.cardText {
		margin: 0.25rem 0 0;
		color: var(--color-text-muted);
		font-size: 0.88rem;
		line-height: 1.4;
	}


	/* Printings list — one row per variant with [- N +] stepper, mirroring
	   the browse-page VariantModal so the same muscle memory works here. */
	.printings {
		list-style: none;
		padding: var(--space-0);
		margin: var(--space-0);
		max-width: 480px;
	}
	.printings li {
		display: grid;
		grid-template-columns: 1fr auto auto auto auto;
		align-items: center;
		gap: var(--space-3);
		padding: 0.45rem 0;
		border-bottom: 1px solid var(--color-border);
	}
	.manual-price {
		background: none;
		border: 1px solid var(--color-border);
		color: var(--color-text-subtle);
		width: 28px;
		height: 28px;
		border-radius: var(--radius-md);
		cursor: pointer;
		font: inherit;
		font-size: var(--text-md);
		line-height: 1;
	}
	.manual-price:hover {
		border-color: var(--color-border-accent);
		color: var(--color-text-accent);
	}
	/* Holds the button's column open for catalog printings, which cannot
	   carry a manual price — same job as .tcgp-spacer. */
	.manual-price-spacer {
		display: inline-block;
		width: 28px;
	}
	.printings .variant {
		color: var(--color-text);
	}
	.vlabel {
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
		min-width: 0;
	}
	.provenance {
		color: var(--color-text-subtle);
		font-size: var(--text-xs);
		line-height: 1.25;
	}
	.printings .market {
		color: var(--color-text-subtle);
		font-size: var(--text-md);
		font-variant-numeric: tabular-nums;
	}
	.tcgp-spacer {
		display: inline-block;
		width: 2.6rem;
	}
	.stepper {
		display: flex;
		align-items: center;
	}
	.step {
		background: var(--color-info-surface);
		border: none;
		color: var(--color-text);
		width: 30px;
		height: 30px;
		cursor: pointer;
		font-size: var(--text-xl);
		line-height: 1;
		padding: var(--space-0);
		border-radius: 0;
	}
	.step:first-child {
		border-radius: var(--radius-md) 0 0 var(--radius-md);
	}
	.step:last-child {
		border-radius: 0 var(--radius-md) var(--radius-md) 0;
	}
	.step:hover:not(:disabled) {
		background: var(--color-accent);
		/* The label has to leave the crimson fill legible — same pairing the
		   Button primitive uses for `variant="primary"`. */
		color: var(--color-on-accent);
	}
	.step:disabled {
		opacity: 0.35;
		cursor: default;
	}
	.count {
		min-width: 34px;
		height: 30px;
		line-height: 30px;
		text-align: center;
		font-size: 0.9rem;
		color: var(--color-text-subtle);
		background: var(--color-surface-page);
	}
	.count.has {
		color: var(--color-success-text);
	}

	table {
		width: 100%;
		max-width: 640px;
		/* Fixed layout so the wide Location select (long binder/deck names)
		   can't force the table past its container — it overflowed the
		   760px card modal otherwise, clipping the rightmost columns and
		   the notes input (dumpstar-8ad). Columns share the table width
		   per the colgroup; selects fill their cell and clip overflow. */
		table-layout: fixed;
		border-collapse: collapse;
		font-size: 0.9rem;
	}
	td select {
		width: 100%;
		max-width: 100%;
		box-sizing: border-box;
	}
	th {
		text-align: left;
		padding: 0.4rem 0.6rem;
		border-bottom: 2px solid var(--color-border);
		color: var(--color-text-subtle);
		font-size: 0.75rem;
		text-transform: uppercase;
	}
	td {
		padding: 0.4rem 0.6rem;
		border-bottom: 1px solid var(--color-border);
		/* Anchor the inline save-confirmation ✓ to each cell. */
		position: relative;
	}
	select {
		background: var(--color-control-surface);
		border: 1px solid var(--color-control-border);
		color: var(--color-control-text);
		border-radius: var(--radius-md);
		padding: 0.15rem;
		font: inherit;
	}
	/* Multi-select copies: bulk-edit bar + checkbox column (dumpstar-0qu). */
	.copybulk {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
		margin-bottom: 0.6rem;
		padding: 0.5rem 0.6rem;
		background: var(--color-surface-accent-wash);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		max-width: 640px;
	}
	.copybulk .count {
		color: var(--color-text);
		font-size: var(--text-md);
		font-weight: var(--weight-semibold);
	}
	.copybulk select {
		max-width: 12rem;
	}
	/* Pushes the Clear control to the far end of the bulk bar. The layout
	   lives on this wrapper rather than on the Button, because a class
	   handed to a primitive is a call-site restyle — the thing the UI
	   vocabulary exists to prevent. */
	.bulkclear {
		margin-left: auto;
		display: inline-flex;
	}
	.selcol {
		text-align: center;
		width: 1.5rem;
	}
	.selcol input {
		cursor: pointer;
	}
	.copyrow.picked td {
		background: var(--color-surface-selected);
	}

	/* Keep each copy and its note visually together: drop the divider between
	   the copy row and its note row; the note row carries the separator. */
	.copyrow td {
		border-bottom: none;
	}
	.noterow td {
		padding-top: 0;
		padding-bottom: 0.5rem;
	}
	.noteinput {
		background: var(--color-control-surface);
		border: 1px solid var(--color-control-border);
		color: var(--color-control-text);
		border-radius: var(--radius-md);
		padding: 0.25rem 0.45rem;
		font: inherit;
		width: 100%;
		box-sizing: border-box;
	}
	.noteinput::placeholder {
		color: var(--color-control-placeholder);
	}
	.tcgp {
		font-size: var(--text-sm);
		color: var(--color-info-text);
		text-decoration: none;
	}
	.tcgp:hover {
		color: var(--color-text-accent);
	}

	/* On a phone the data tables reflow to stacked label:value blocks so
	   they fit the modal instead of forcing it wider. */
	@media (max-width: 540px) {
		/* Stack art over info on phones (mirrors DD's
		   card-detail-layout). Setting flex-direction: column means the
		   row-wise min-width of .info never wedges info beside art —
		   info always flows below, full-width. */
		.detail {
			flex-direction: column;
			align-items: center;
		}
		.info {
			min-width: 0;
			width: 100%;
		}
		.art img {
			width: 320px;
			max-width: 100%;
		}
		table {
			max-width: 100%;
			/* Drop the table formatting context entirely on mobile: the rows
			   stack as blocks below, and leaving the <table> as display:table
			   with table-layout:fixed + the <colgroup> wraps the block tbody in
			   an anonymous cell pinned to column 1 (~19%), collapsing the whole
			   thing into an unusable strip (dumpstar-9i5). */
			display: block;
			table-layout: auto;
		}
		colgroup {
			display: none;
		}
		thead {
			display: none;
		}
		tbody,
		tr,
		td {
			display: block;
		}
		tr {
			border: 1px solid var(--color-border);
			border-radius: var(--radius-lg);
			margin-bottom: 0.6rem;
			padding: 0.1rem 0.6rem;
		}
		td {
			display: flex;
			justify-content: space-between;
			align-items: center;
			gap: var(--space-4);
			padding: 0.35rem 0;
			border-bottom: none;
		}
		td::before {
			content: attr(data-label);
			color: var(--color-text-subtle);
			font-size: var(--text-xs);
			text-transform: uppercase;
			flex-shrink: 0;
		}
		td select {
			flex: 1;
			min-width: 0;
			/* Override the desktop fixed-table width:100% so the select shares
			   the row with its data-label instead of overflowing. */
			width: auto;
		}
	}

	/* Price-history header's owned-only / show-all toggle, rendered into
	   SectionHeader's `actions` slot. */
	.showall {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
		cursor: pointer;
		white-space: nowrap;
	}

	/* User-added printings (the missing-variant escape hatch) render
	   with an italic variant label and a small color pip to its left. */
	.printings li.user-added .vlabel .variant {
		font-style: italic;
	}
	.vline {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
	}
	.user-pip {
		display: inline-block;
		width: 8px;
		height: 8px;
		border-radius: var(--radius-round);
		background: var(--color-text-decorative);
		flex-shrink: 0;
	}
</style>
