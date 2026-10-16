<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api, SearchQueryError } from '$lib/api';
	import { facetHref } from '$lib/facets';
	import { sortAspects } from '$lib/aspects';
	import RarityMark from '$lib/components/RarityMark.svelte';
	import CostMark from '$lib/components/CostMark.svelte';
	import AspectIcon from '$lib/components/AspectIcon.svelte';
	import SetLogo from '$lib/components/SetLogo.svelte';
		import { CONDITIONS } from '$lib/conditions';
	import { conditionMultiplier } from '$lib/conditions.svelte';
	import { money, count, printingLabel, printingTag, jsonList } from '$lib/format';
	import { stackOffsets, windowOf } from '$lib/virtual';

	// Foil shimmer treatment for Foil-finish printings.
	function isFoil(finish: string): boolean {
		return finish === 'Foil';
	}
	import CardModal from '$lib/components/CardModal.svelte';
	import ValueHistoryModal from '$lib/components/ValueHistoryModal.svelte';
	import DeathStar from '$lib/components/DeathStar.svelte';
	import { Button, EmptyState, SectionHeader, Toolbar } from '$lib/components/ui';
	import type { CollectionRow } from '$lib/types/CollectionRow';
	import type { SearchRow } from '$lib/types/SearchRow';
	import type { SearchVocabulary } from '$lib/types/SearchVocabulary';
	import type { Binder } from '$lib/types/Binder';
	import type { Deck } from '$lib/types/Deck';
	import StatFrame from '$lib/components/StatFrame.svelte';
	import { COLUMNS, PHONE_MAX_WIDTH, anyAdjusted, parseHidden, visibleColumns } from '$lib/collectionColumns';

	// Server-side search results (one row per printing, owned or not). The
	// page's existing rendering is per-copy, so owned printings are flattened
	// back into per-copy rows (`rows`); unowned printings (owned_count === 0)
	// drive the dimmed "missing" tiles (see `displayRows`).
	//
	// This is the WHOLE result set, not a page of it (pd-7z4o). A catalog-wide
	// query matches 56,635 printings and that is exactly what arrives: the
	// response is compressed, so those rows travel as ~3 MB rather than 44
	// (pd-2r0p), and the reason the tab survived neither before was never the
	// payload — it was 289,612 DOM nodes and a 457 MB heap (pd-5vgp). So the
	// rows are held here as JS objects and only the slice under the viewport is
	// put in the document. See the virtual-scroller block below.
	//
	// `$state.raw` and not `$state`, and at this size it is not a micro-
	// optimisation: plain `$state` wraps every object it holds in a deep
	// reactive proxy, which on 56,635 rows of twenty fields measured **390 MB**
	// of retained heap and a 3.1 s first paint — within sight of the 457 MB
	// that crashed the tab in the first place (pd-5vgp). Raw, the same result
	// is **48 MB** and 1.3 s. Nothing here mutates a row in place; a search
	// replaces the whole array, which is exactly what raw state is for.
	let searchRows = $state.raw<SearchRow[]>([]);
	// Printings the query matches in total — equal to `searchRows.length`, and
	// kept as its own field because it is the endpoint that says so.
	let searchTotal = $state(0);

	// The header figure beside the count. It comes from the endpoint, like the
	// count does (pd-2g84): it is the condition-adjusted value of every OWNED
	// copy the query matches, whether or not this client is holding them all.
	//
	// Summing it here is what it used to do, and that could only ever be right
	// while the page held the whole result — under paging the figure had to be
	// withheld unless the page WAS the result. It also made the page a second
	// implementation of what a collection is worth, and the two disagreed: the
	// chart this number opens values `status = 'owned'` copies, and a reduce
	// over `filtered` counts the sold and ordered ones the list draws beside
	// them. `null` is "no figure", never $0.00 — see `SearchPage.total_value`.
	let searchTotalValue = $state<number | null>(null);

	let loading = $state(true);
	let error = $state<string | null>(null);
	// A query-language parse error, shown under the search box with a caret.
	let searchError = $state<{ message: string; position: number } | null>(null);

	// One rendered row per owned copy. Deliberately NOT CollectionRow: the
	// search payload carries only the fields the list draws (pd-lk8v), so
	// claiming that shape here would promise `artist` the server
	// no longer sends. Anchored to CollectionRow by Omit so a field added
	// there still has to be answered for here.
	type ListRow = Omit<CollectionRow, 'artist' | 'variant_description'>;

	const rows = $derived<ListRow[]>(
		searchRows.flatMap((sr) =>
			sr.copies.map((cp) => ({
				id: cp.id,
				printing_id: sr.printing_id,
				condition: cp.condition,
				language: cp.language,
				purchase_price: cp.purchase_price,
				sale_price: null,
				acquired_at: cp.acquired_at,
				source: '',
				notes: null,
				status: cp.status,
				graded: cp.graded,
				binder_id: cp.binder_id,
				deck_id: cp.deck_id,
				variant: sr.variant,
				finish: sr.finish,
				card_id: sr.card_id,
				set_code: sr.set_code,
				set_name: sr.set_name,
				number: sr.number,
				name: sr.name,
				subtitle: sr.subtitle,
				rarity: sr.rarity,
				card_type: sr.card_type,
				cost: sr.cost,
				power: sr.power,
				hp: sr.hp,
				aspects: sr.aspects,
				arenas: sr.arenas,
				market_price: sr.market_price,
				image_url: sr.image_url
			}))
		)
	);

	// Debounced search. Initial value comes from ?q= so clickable facets
	// on the card-detail page (artist, set, aspect, rarity, …) can
	// land here with the filter pre-applied.
	const initialQuery =
		typeof window !== 'undefined' ? (page.url.searchParams.get('q') ?? '') : '';
	let searchRaw = $state(initialQuery);
	// The committed (debounced) query sent to the server. Empty = owned default.
	let query = $state(initialQuery.trim());
	let debounce: ReturnType<typeof setTimeout>;
	let searchInput = $state<HTMLInputElement | undefined>();

	// --- Autocomplete: keyword aliases for a bare token, is:/has: values
	//     after the colon. Vocabulary comes from the data-driven registry
	//     (api.searchKeywords); nothing here is hardcoded. (dumpstar-cx1) ---
	type AcItem = { insert: string; label: string; hint: string };
	const HAS_VALUES = ['text', 'subtitle', 'artist'];
	let vocab = $state.raw<SearchVocabulary | null>(null);
	let acFocused = $state(false);
	let acDismissed = $state(false);
	let acIndex = $state(0);

	// The trailing whitespace-delimited token (what the caret is completing)
	// and the text before it.
	function tokenInfo(raw: string): { prefix: string; token: string } {
		const m = raw.match(/(\S*)$/);
		const token = m ? m[0] : '';
		return { prefix: raw.slice(0, raw.length - token.length), token };
	}

	const suggestions = $derived.by<AcItem[]>(() => {
		const v = vocab;
		if (!v) return [];
		const { token } = tokenInfo(searchRaw);
		if (!token) return [];
		const neg = token.startsWith('-') ? '-' : '';
		const bare = neg ? token.slice(1) : token;
		const colon = bare.indexOf(':');
		if (colon >= 0) {
			const kw = bare.slice(0, colon).toLowerCase();
			const partial = bare.slice(colon + 1).toLowerCase();
			let pool: { v: string; h: string }[] = [];
			if (kw === 'is') pool = v.flags.map((f) => ({ v: f.flag, h: f.help ?? '' }));
			else if (kw === 'has') pool = HAS_VALUES.map((x) => ({ v: x, h: '' }));
			else return [];
			return pool
				.filter((x) => x.v.toLowerCase().startsWith(partial))
				.slice(0, 8)
				.map((x) => ({ insert: `${neg}${kw}:${x.v}`, label: `${kw}:${x.v}`, hint: x.h }));
		}
		const lower = bare.toLowerCase();
		const items: AcItem[] = [];
		for (const k of v.keywords) {
			const alias = k.aliases.find((a) => a.toLowerCase().startsWith(lower));
			if (alias) {
				const op = k.operators.includes(':') ? ':' : (k.operators[0] ?? ':');
				items.push({ insert: `${neg}${alias}${op}`, label: `${alias}${op}`, hint: k.help ?? '' });
			}
		}
		return items.slice(0, 8);
	});

	const acOpen = $derived(acFocused && !acDismissed && suggestions.length > 0);

	function acceptSuggestion(item: AcItem) {
		const { prefix } = tokenInfo(searchRaw);
		acDismissed = false;
		acIndex = 0;
		onSearch(prefix + item.insert);
		searchInput?.focus();
	}

	function onSearchKeydown(e: KeyboardEvent) {
		if (!acOpen) return;
		const n = suggestions.length;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			acIndex = (acIndex + 1) % n;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			acIndex = (acIndex - 1 + n) % n;
		} else if (e.key === 'Enter') {
			e.preventDefault();
			acceptSuggestion(suggestions[Math.min(acIndex, n - 1)]);
		} else if (e.key === 'Escape') {
			e.preventDefault();
			acDismissed = true;
		}
	}
	// Reflect the whole view — query and All-cards — in the URL, so refreshes
	// and the back button keep state.
	// goto(replaceState:true), NOT $app/navigation's replaceState: the latter
	// only repaints the address bar and stashes the *stale* page.url as the
	// entry's restore key, so a forward nav (card facet link) + Back drops the
	// ?q=. goto updates page.url so Back restores it. keepFocus so the search
	// box doesn't blur mid-type.
	//
	// One writer for both params: they change together and a second writer
	// would race this one on goto. There is no `offset` among them any more —
	// the list is one scrollable result, so where you are in it is a scroll
	// position (remembered below), not a slice of the URL.
	function syncUrl() {
		if (typeof window === 'undefined') return;
		const url = new URL(window.location.href);
		const q = searchRaw.trim();
		if (q) url.searchParams.set('q', q);
		else url.searchParams.delete('q');
		if (allCards) url.searchParams.set('all', '1');
		else url.searchParams.delete('all');
		if (url.href !== window.location.href) {
			void goto(url, { replaceState: true, keepFocus: true, noScroll: true });
		}
	}

	function onSearch(value: string) {
		searchRaw = value;
		// Typing reopens the autocomplete and resets the highlight.
		acDismissed = false;
		acIndex = 0;
		clearTimeout(debounce);
		debounce = setTimeout(() => {
			query = value.trim();
			// A different result set is read from the top.
			forgetScroll();
			syncUrl();
		}, 200);
	}

	// Re-sync from the URL whenever it changes — covers the case where the
	// user clicks a facet link in the card modal while already on
	// /collection. SvelteKit's client router updates the URL but doesn't
	// remount us, so the initial-value read above never re-runs. `untrack`
	// keeps this effect from refiring when the user types into the box
	// (which writes searchRaw + the URL itself); only an external URL
	// change triggers the resync.
	$effect(() => {
		const q = page.url.searchParams.get('q') ?? '';
		const all = page.url.searchParams.get('all') === '1';
		untrack(() => {
			if (q !== searchRaw) {
				clearTimeout(debounce);
				searchRaw = q;
				query = q.trim();
				// Close any open modal so the filtered list is visible.
				selectedCard = null;
			}
			// The "All cards" param has to resync from the URL too. A
			// client-side facet nav (e.g. a card-modal link carrying &all=1)
			// updates the URL without remounting, so `initialAllCards` never
			// re-runs — without this, the catalog stayed owned-only until a
			// manual reload even though the URL already said all=1.
			if (all !== allCards) {
				allCards = all;
			}
		});
	});

	// "All cards" toggle widens the server search from owned-only to the whole
	// catalog (include_unowned=1). Unowned printings come back with
	// owned_count === 0 and render as dimmed "missing" tiles — the same visual
	// treatment /browse/[set] uses for unowned binder slots.
	const initialAllCards =
		typeof window !== 'undefined' && page.url.searchParams.get('all') === '1';
	let allCards = $state(initialAllCards);
	function toggleAllCards() {
		allCards = !allCards;
		// Widening owned-only to the whole catalog is a different result set,
		// so it is read from the top.
		forgetScroll();
		syncUrl();
	}

	/** Fetch the whole result for the current query + toggle + sort. */
	async function runSearch() {
		loading = true;
		error = null;
		searchError = null;
		try {
			// `limit: 'all'` — every matching row, because this page holds the
			// result and renders a window of it. Only a caller that can hold
			// the whole thing may ask; this one can (see `searchRows`).
			//
			// `sort`/`dir` are always sent, so the control wins over an
			// `order:` written into the query itself. That is not a change:
			// the control won before too, by re-sorting the rows after they
			// arrived. What changed is that it now wins where the rows are
			// chosen, so the order the control claims is the order you get —
			// and it has to stay server-side, because a sort applied to a
			// window would order what the window happens to hold.
			const res = await api.collectionSearch(query, sortKey, sortDir, allCards, 'all');
			searchRows = res.rows;
			searchTotal = res.total;
			searchTotalValue = res.total_value;
		} catch (e) {
			if (e instanceof SearchQueryError) {
				searchError = { message: e.message, position: e.position };
				searchRows = [];
				searchTotal = 0;
				searchTotalValue = null;
			} else {
				error = e instanceof Error ? e.message : String(e);
			}
		} finally {
			loading = false;
			restoreScroll();
		}
	}

	// Re-search whenever anything the server decides changes — the query, the
	// All-cards toggle, or the sort (also fires once on mount). The sort is in
	// that list because it is the SERVER'S: it decides which rows the ORDER BY
	// puts where, and the client only ever draws them in that order.
	$effect(() => {
		void query;
		void allCards;
		void sortKey;
		void sortDir;
		runSearch();
	});

	// --- Multi-select bulk operations. ---
	let binders = $state.raw<Binder[]>([]);
	let decks = $state.raw<Deck[]>([]);
	let selectMode = $state(false);
	// Selected copies, keyed by copy id — which is what makes a selection
	// survive the window. The tile that carried a copy is gone from the DOM
	// the moment it scrolls out, so the selection remembers the little it
	// needs about each one (the card, for "add to wishlist") rather than
	// looking it back up in the document. Keyed by id and not by position for
	// the same reason: nothing here is an index into what is rendered.
	type Picked = { card_id: string; printing_id: string };
	// Raw, like `searchRows`: every mutation below builds a new Map and assigns
	// it, so there is nothing for a deep proxy to observe.
	let selected = $state.raw(new Map<number, Picked>());
	let busy = $state(false);

	// Grid (card images) vs. table view — persisted across reloads in
	// localStorage so refreshes don't snap back to the default.
	function readStoredView(): 'grid' | 'table' {
		if (typeof window === 'undefined') return 'grid';
		const v = localStorage.getItem('collection.view');
		return v === 'table' || v === 'grid' ? v : 'grid';
	}
	let view = $state<'grid' | 'table'>(readStoredView());
	$effect(() => {
		if (typeof window !== 'undefined') {
			localStorage.setItem('collection.view', view);
		}
	});
	let selectedCard = $state<{ set: string; number: string } | null>(null);

	// Table columns the reader has hidden — persisted like the view and the
	// sort. Adjusted starts hidden and is drawn only when some row is adjusted.
	// A phone keeps its own choice, under its own key, and starts with Name and
	// Type only (as deckdumpster's mobile table). Written when the reader
	// chooses, not on load, so an untouched default can still move.
	const phoneTable =
		typeof window !== 'undefined' && window.matchMedia(`(max-width: ${PHONE_MAX_WIDTH}px)`).matches;
	const hiddenKey = phoneTable ? 'collection.hiddenColumns.phone' : 'collection.hiddenColumns';
	let hiddenCols = $state<string[]>(
		parseHidden(typeof window === 'undefined' ? null : localStorage.getItem(hiddenKey), phoneTable)
	);
	let columnsOpen = $state(false);
	function toggleColumn(key: string) {
		hiddenCols = hiddenCols.includes(key)
			? hiddenCols.filter((k) => k !== key)
			: [...hiddenCols, key];
		localStorage.setItem(hiddenKey, JSON.stringify(hiddenCols));
	}

	// Burger menu (Export CSV, Select) lives in the sticky top bar.
	let menuOpen = $state(false);
	// Collection value-over-time chart modal (dumpstar-e1vo).
	let valueOpen = $state(false);
	function closeMenu() {
		menuOpen = false;
	}

	// Column sort — persisted across reloads in localStorage so refreshes
	// don't snap back to the default (mirrors the view-mode persistence
	// above), and sent to the server on every search.
	//
	// These are the endpoint's own key names, not a private vocabulary with a
	// translation table in the middle: the sort is executed in SQL
	// (`search::order_sql`), because the browser renders a window of the
	// result and sorting that window would order the wrong rows.
	//
	// Every one of them is a stored scalar in a single database, which is what
	// lets an index satisfy the ordering. `adj` and `value` are not, and are no
	// longer here: both were computed by a subquery joining the collection to
	// the shared catalog's prices across the ATTACH boundary, so ordering by
	// either meant sorting the whole match set before LIMIT could discard any
	// of it (pd-tjym). The server refuses them now rather than quietly ordering
	// by name, so this list and its arms have to agree — which is also why a
	// stored `collection.sortKey` outside it falls back to `name` below rather
	// than being sent.
	const SORT_KEYS = [
		'name',
		'number',
		'set',
		'rarity',
		'type',
		'cost',
		'power',
		'hp',
		'price',
		'qty',
		'added'
	];
	function readStoredSort(): { key: string; dir: 'asc' | 'desc' } {
		if (typeof window === 'undefined') return { key: 'name', dir: 'asc' };
		const k = localStorage.getItem('collection.sortKey');
		const d = localStorage.getItem('collection.sortDir');
		return {
			key: k && SORT_KEYS.includes(k) ? k : 'name',
			dir: d === 'desc' ? 'desc' : 'asc'
		};
	}
	const _storedSort = readStoredSort();
	let sortKey = $state(_storedSort.key);
	let sortDir = $state<'asc' | 'desc'>(_storedSort.dir);
	$effect(() => {
		if (typeof window !== 'undefined') {
			localStorage.setItem('collection.sortKey', sortKey);
			localStorage.setItem('collection.sortDir', sortDir);
		}
	});

	function sortBy(key: string) {
		if (sortKey === key) {
			sortDir = sortDir === 'asc' ? 'desc' : 'asc';
		} else {
			sortKey = key;
			// Counts and money default to high→low; everything else low→high.
			sortDir = key === 'qty' || key === 'price' ? 'desc' : 'asc';
		}
		// Re-ordering the whole result set puts different rows where you were
		// looking. Start at the top of the new order.
		forgetScroll();
	}

	// Set by the modal's onMutate when a copy is added/removed/edited while
	// the modal is open. Closing only re-runs the search when this is set —
	// merely viewing a card (then closing) leaves the list untouched.
	let cardDirty = $state(false);

	/** Close the modal, re-running the search only if the modal mutated copies. */
	async function closeCard() {
		selectedCard = null;
		if (cardDirty) {
			cardDirty = false;
			await runSearch();
		}
	}

	onMount(async () => {
		// The $effect above runs the initial search; here we only load the
		// binder/deck lists used by the bulk-assign menus.
		try {
			[binders, decks, vocab] = await Promise.all([
				api.binders(),
				api.decks(),
				api.searchKeywords()
			]);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	});

	// The server applies the query now; `filtered` is just the owned copies it
	// returned (kept as a name so the rest of the page is unchanged).
	const filtered = $derived(rows);

	function toggleSelectMode() {
		selectMode = !selectMode;
		if (!selectMode) selected = new Map();
	}

	// === Table view: aggregate per-copy rows so the table can show Qty. ===

	type AggRow = {
		key: string;
		ids: number[];
		qty: number;
		paid_total: number | null;
		/** Per-copy raw Near Mint market price (every row in the group shares
		    a printing, so this is condition-independent). */
		nm_unit: number | null;
		/** Per-copy condition-adjusted market price (nm_unit × the copy's
		    condition multiplier) — the Adj. column. There is no per-group total
		    beside it any more: the Value column left with its sort (pd-tjym),
		    and the result set's total is `searchTotalValue`, which the server
		    computes over the whole result rather than over a group. */
		market_unit: number | null;
		printing_id: string;
		card_id: string;
		set_code: string;
		set_name: string;
		number: string;
		name: string;
		subtitle: string | null;
		rarity: string | null;
		card_type: string | null;
		/** Resource cost, power and hit points (null where the card has none). */
		cost: number | null;
		power: number | null;
		hp: number | null;
		/** JSON array string, e.g. `["Vigilance","Heroism"]`. */
		aspects: string | null;
		variant: string;
		finish: string;
		condition: string;
		status: string;
		image_url: string | null;
	};

	function aggregate(input: ListRow[]): AggRow[] {
		const map = new Map<string, AggRow>();
		for (const r of input) {
			const key = `${r.printing_id}|${r.condition}|${r.status}`;
			// market_price is always NM market from the API; condition-adjust
			// it here so the Adj. column reflects the copy's actual worth,
			// not the NM headline (dumpstar-qtp). Every copy in a group
			// shares a condition, so one figure covers the group.
			const condValue =
				r.market_price != null ? r.market_price * conditionMultiplier(r.condition) : null;
			const existing = map.get(key);
			if (existing) {
				existing.ids.push(r.id);
				existing.qty += 1;
				if (r.purchase_price != null) {
					existing.paid_total = (existing.paid_total ?? 0) + r.purchase_price;
				}
			} else {
				map.set(key, {
					key,
					ids: [r.id],
					qty: 1,
					paid_total: r.purchase_price,
					nm_unit: r.market_price,
					market_unit: condValue,
					printing_id: r.printing_id,
					card_id: r.card_id,
					set_code: r.set_code,
					set_name: r.set_name,
					number: r.number,
					name: r.name,
					subtitle: r.subtitle,
					rarity: r.rarity,
					card_type: r.card_type,
					cost: r.cost,
					power: r.power,
					hp: r.hp,
					aspects: r.aspects,
					variant: r.variant,
					finish: r.finish,
					condition: r.condition,
					status: r.status,
					image_url: r.image_url
				});
			}
		}
		return [...map.values()];
	}

	// Unowned printings become qty-0 AggRows so owned and unowned draw the same
	// way in one list (dumpstar-ffq). They carry the catalog market price
	// (no condition to adjust → nm_unit === market_unit).
	function unownedRow(sr: SearchRow): AggRow {
		return {
			key: `missing|${sr.printing_id}`,
			ids: [],
			qty: 0,
			paid_total: null,
			nm_unit: sr.market_price,
			market_unit: sr.market_price,
			printing_id: sr.printing_id,
			card_id: sr.card_id,
			set_code: sr.set_code,
			set_name: sr.set_name,
			number: sr.number,
			name: sr.name,
			subtitle: sr.subtitle,
			rarity: sr.rarity,
			card_type: sr.card_type,
			cost: sr.cost,
			power: sr.power,
			hp: sr.hp,
			aspects: sr.aspects,
			variant: sr.variant,
			finish: sr.finish,
			condition: '',
			status: 'unowned',
			image_url: sr.image_url
		};
	}

	/** The page's owned groups, indexed by the printing they came from. */
	const groupsByPrinting = $derived.by(() => {
		const m = new Map<string, AggRow[]>();
		for (const a of aggregate(filtered)) {
			const list = m.get(a.printing_id);
			if (list) list.push(a);
			else m.set(a.printing_id, [a]);
		}
		return m;
	});

	/** Every row both views draw, in the order the server returned them.

	    There is no client-side sort left. The order is the ORDER BY the server
	    executed for `sortKey`/`sortDir`, which is the only order that can be
	    right when the document holds a window of up to 56,635 matches —
	    sorting the window would sort the wrong rows. Walking `searchRows`
	    rather than concatenating owned and unowned is what keeps that order
	    intact: a missing printing lands exactly where the sort put it, next to
	    the owned ones around it, instead of in a clump at the end. */
	const displayRows = $derived.by<AggRow[]>(() => {
		const out: AggRow[] = [];
		for (const sr of searchRows) {
			if (sr.owned_count === 0) out.push(unownedRow(sr));
			else out.push(...(groupsByPrinting.get(sr.printing_id) ?? []));
		}
		return out;
	});


	// === Grid grouping ================================================
	//
	// Bigger art costs scroll, and scroll is only worth paying for if what
	// you scroll through is organised. So the grid cuts itself into labelled
	// sections along whatever the active sort already orders on, whenever
	// that field is a category a person browses by — set, rarity, class,
	// card type, or a name's initial. The continuous fields (#, NM) group
	// into nothing useful, so they stay one flat run.
	//
	// Every label is read straight off the row's own fields. No new
	// taxonomy, no bucket table: a heading here is a value the catalog
	// already stores, which is why this needs no schema of its own.
	function groupLabel(a: AggRow): string | null {
		switch (sortKey) {
			case 'name': {
				const c = a.name.trim().charAt(0).toUpperCase();
				return c >= 'A' && c <= 'Z' ? c : '#';
			}
			case 'set':
				return a.set_name;
			case 'rarity':
				return a.rarity ?? 'No rarity';
			case 'type':
				return a.card_type ?? 'Other';
			case 'cost':
				return a.cost == null ? 'No cost' : `Cost ${a.cost}`;
			default:
				return null;
		}
	}

	/** `displayRows`, cut into runs of equal group label — as index ranges into
	    it, not as copies of it. Concatenating the runs reproduces `displayRows`
	    exactly, so the grid's tile order is untouched and owned and unowned
	    printings still interleave by the sort key.

	    Ranges rather than row arrays because the runs now cover the whole
	    result: slicing 56,635 rows into a few thousand little arrays would
	    double the heap to describe an order the flat array already has. */
	const gridSections = $derived.by(() => {
		const out: { label: string | null; from: number; to: number }[] = [];
		if (view !== 'grid') return out;
		for (let i = 0; i < displayRows.length; i++) {
			const label = groupLabel(displayRows[i]);
			const last = out[out.length - 1];
			if (last && last.label === label) last.to = i + 1;
			else out.push({ label, from: i, to: i + 1 });
		}
		return out;
	});

	// === Virtual scroller (pd-7z4o) ===================================
	//
	// The result is held above in full; the document holds a window of it.
	// Everything below is the arithmetic that keeps those two agreeing — the
	// spacers standing in for what isn't rendered are what make the scrollbar,
	// the scroll position and every in-page offset describe the whole result
	// rather than the window.
	//
	// The list is a stack of BLOCKS: one table row, or in the grid either a
	// section heading or one full row of tiles. `$lib/virtual` turns block
	// heights into offsets and offsets into a rendered range; nothing here
	// decides what a block LOOKS like, and nothing there touches the DOM.

	/** One block of the grid: a heading, or one row of `cols` tiles given as a
	    range into `displayRows`. */
	type GridBlock =
		| { kind: 'header'; label: string; count: number }
		| { kind: 'tiles'; from: number; to: number };

	const gridBlocks = $derived.by<GridBlock[]>(() => {
		const out: GridBlock[] = [];
		for (const sec of gridSections) {
			if (sec.label !== null) {
				out.push({ kind: 'header', label: sec.label, count: sec.to - sec.from });
			}
			for (let i = sec.from; i < sec.to; i += cols) {
				out.push({ kind: 'tiles', from: i, to: Math.min(i + cols, sec.to) });
			}
		}
		return out;
	});

	// Geometry, measured off the rendered document rather than mirrored from
	// the stylesheet — tile size, column count and gap are all decided by CSS
	// and by the viewport, and a copy of them here would be a second opinion
	// that drifts. The values below are seeds for the very first frame only:
	// that frame is re-windowed from real measurements before it can be
	// scrolled.
	let tileH = $state(210);
	let headerH = $state(44);
	let gridGap = $state(16);
	let cols = $state(1);
	let rowH = $state(48);

	/** Viewports of slack rendered either side of the visible one, so a flung
	    scroll lands on tiles that are already there. The rendered count is a
	    multiple of what fits on screen — never of how much matched. */
	const OVERSCAN = 1;

	/** The list container: the grid, or the table's tbody. */
	let listEl = $state<HTMLElement | undefined>();
	let scrollY = $state(0);
	let viewportH = $state(0);
	/** Where the list starts in the document, so window coordinates become
	    list coordinates. */
	let listTop = $state(0);

	const winTop = $derived(scrollY - listTop - OVERSCAN * viewportH);
	const winBottom = $derived(scrollY - listTop + (1 + OVERSCAN) * viewportH);

	const gridOffsets = $derived(
		stackOffsets(
			gridBlocks.length,
			(i) => (gridBlocks[i].kind === 'header' ? headerH : tileH),
			gridGap
		)
	);
	const gridWindow = $derived(windowOf(gridOffsets, gridGap, winTop, winBottom));
	/** The blocks to render, carrying their absolute index so scrolling by one
	    row rebuilds one block rather than the whole window. */
	const gridVisible = $derived.by(() => {
		const out: { i: number; block: GridBlock }[] = [];
		for (let i = gridWindow.start; i < gridWindow.end; i++) out.push({ i, block: gridBlocks[i] });
		return out;
	});

	const tableOffsets = $derived(
		stackOffsets(view === 'table' ? displayRows.length : 0, () => rowH, 0)
	);
	const tableWindow = $derived(windowOf(tableOffsets, 0, winTop, winBottom));
	// The Aspects column is sized for the card with the most aspects in the
	// whole result, not the rows the virtual window happens to hold.
	const maxAspects = $derived(
		Math.max(1, ...displayRows.map((r) => jsonList(r.aspects).length)),
	);
	const tableVisible = $derived(displayRows.slice(tableWindow.start, tableWindow.end));
	/** Columns a spacer row has to span. */
	const adjustedPresent = $derived(anyAdjusted(displayRows));
	const shownCols = $derived(visibleColumns(hiddenCols, adjustedPresent));
	const showCol = (k: string) => shownCols.includes(k);
	const tableCols = $derived((selectMode ? 1 : 0) + 1 + shownCols.length);
	/** Hidden columns spend no width in the table's min-width. */
	const colVars = $derived(
		[
			`--aspect-n: ${maxAspects}`,
			...COLUMNS.filter((c) => !shownCols.includes(c.key)).map((c) => `--col-${c.key}:0rem`)
		].join(';')
	);


	/** Read the geometry the window arithmetic needs out of the rendered DOM. */
	function measure() {
		const el = listEl;
		if (!el || typeof window === 'undefined') return;
		listTop = el.getBoundingClientRect().top + window.scrollY;
		if (view === 'grid') {
			const style = getComputedStyle(el);
			const tracks = style.gridTemplateColumns.split(' ').filter((t) => t !== '').length;
			if (tracks > 0) cols = tracks;
			const gap = parseFloat(style.rowGap);
			if (!Number.isNaN(gap)) gridGap = gap;
			const tile = el.querySelector('.cardtile');
			if (tile) tileH = tile.getBoundingClientRect().height;
			// Headings only exist for the sorts that group; the last measured
			// value stands until one is on screen again.
			const head = el.querySelector('.groupheader');
			if (head) headerH = head.getBoundingClientRect().height;
		} else {
			// One height for every row, by construction — see `.namebody`.
			const row = el.querySelector('tr:not(.vspace)');
			if (row) rowH = row.getBoundingClientRect().height;
		}
	}

	// Re-measure after every render that can change the geometry: a new view, a
	// new sort (headings appear and disappear), a new result, select mode (an
	// extra column). Deliberately reads none of the values it writes — that
	// would be a cycle.
	$effect(() => {
		void view;
		void sortKey;
		void selectMode;
		void displayRows.length;
		measure();
	});

	// Scroll and viewport size drive the window. One rAF per scroll burst, so
	// a fling costs one recompute per frame rather than one per event.
	$effect(() => {
		if (typeof window === 'undefined') return;
		let frame = 0;
		const onScroll = () => {
			if (frame) return;
			frame = requestAnimationFrame(() => {
				frame = 0;
				scrollY = window.scrollY;
				rememberScroll();
			});
		};
		const onResize = () => {
			viewportH = window.innerHeight;
			measure();
		};
		window.addEventListener('scroll', onScroll, { passive: true });
		window.addEventListener('resize', onResize);
		onResize();
		scrollY = window.scrollY;
		return () => {
			cancelAnimationFrame(frame);
			window.removeEventListener('scroll', onScroll);
			window.removeEventListener('resize', onResize);
		};
	});

	// A width change re-flows the grid's columns without necessarily resizing
	// the window — a scrollbar appearing does it. Height changes are ours (the
	// spacers) and are ignored, or this would observe its own output.
	$effect(() => {
		const el = listEl;
		if (!el || typeof ResizeObserver === 'undefined') return;
		let width = el.getBoundingClientRect().width;
		const ro = new ResizeObserver((entries) => {
			const next = entries[0].contentRect.width;
			if (next === width) return;
			width = next;
			measure();
		});
		ro.observe(el);
		return () => ro.disconnect();
	});

	// --- Where the reader was --------------------------------------------
	//
	// Under paging this was `?offset=`. It is a scroll position now, which is
	// not URL state — it is per-tab and per-view, and it dies with the tab.
	// Without it, Back out of a card's facet search lands at the top of 56,635
	// rows instead of where you were reading.

	function scrollKey(): string {
		return `collection.scroll:${view}|${query}|${sortKey}|${sortDir}|${allCards ? 1 : 0}`;
	}
	let scrollSave: ReturnType<typeof setTimeout>;
	function rememberScroll() {
		clearTimeout(scrollSave);
		scrollSave = setTimeout(() => {
			sessionStorage.setItem(scrollKey(), String(window.scrollY));
		}, 150);
	}
	/** A different result set is read from the top: forget where the reader was
	    in it, and let the restore after the search put them there. */
	function forgetScroll() {
		if (typeof window === 'undefined') return;
		clearTimeout(scrollSave);
		sessionStorage.removeItem(scrollKey());
	}
	/** Put the reader back where they were in this result. Two frames: one for
	    the first window to render, one for the measurement it triggers to give
	    the spacers their real height — the document has to be tall enough to
	    scroll to before we scroll to it. */
	function restoreScroll() {
		if (typeof window === 'undefined') return;
		const saved = Number(sessionStorage.getItem(scrollKey()) ?? 0);
		requestAnimationFrame(() => requestAnimationFrame(() => window.scrollTo({ top: saved })));
	}

	function groupChecked(ids: number[]): boolean {
		// An unowned printing has no copies — never "checked" (empty .every is true).
		return ids.length > 0 && ids.every((id) => selected.has(id));
	}
	function toggleGroup(a: AggRow) {
		const all = groupChecked(a.ids);
		const next = new Map(selected);
		for (const id of a.ids) {
			if (all) next.delete(id);
			else next.set(id, { card_id: a.card_id, printing_id: a.printing_id });
		}
		selected = next;
	}
	function openGroup(a: AggRow) {
		if (selectMode) toggleGroup(a);
		else selectedCard = { set: a.set_code, number: a.number };
	}

	// Table view: only the Name cell opens the modal; every other column is a
	// DSL-appropriate facet search (rarity:, set:, type:, …) reusing the same
	// builder as the card-page links (dumpstar-ozm). In select mode the
	// whole row still toggles selection, so these handlers no-op and let the
	// click bubble to the row's selection handler.
	function openCardCell(e: MouseEvent, a: AggRow) {
		if (selectMode) return;
		e.stopPropagation();
		selectedCard = { set: a.set_code, number: a.number };
	}
	function facetCell(e: MouseEvent, field: string, value: string | null | undefined) {
		if (selectMode) return;
		e.stopPropagation();
		if (value) void goto(facetHref(field, value));
	}

	// Non-owned statuses surface as a small badge next to the card name —
	// the column itself is gone (no point spamming "owned" on every row).
	function statusBadge(status: string): string | null {
		switch (status) {
			case 'owned':
				return null;
			case 'ordered':
				return 'ORD';
			case 'listed':
				return 'LST';
			case 'sold':
				return 'SLD';
			case 'traded':
				return 'TRD';
			case 'gifted':
				return 'GFT';
			case 'lost':
				return 'LOST';
			case 'removed':
				return 'RMV';
			default:
				return status.slice(0, 3).toUpperCase();
		}
	}

	// The header checkbox in the table selects/clears every owned row in the
	// RESULT — which is now every row the client holds, not merely the ones
	// currently in the document. Clearing is a delete of those ids rather than
	// a new empty selection, so a selection made under a different query
	// survives it.
	const ownedInResult = $derived(displayRows.filter((a) => a.ids.length > 0));
	const tableAllSelected = $derived(
		ownedInResult.length > 0 && ownedInResult.every((a) => groupChecked(a.ids))
	);
	function toggleTableAll() {
		const all = tableAllSelected;
		const next = new Map(selected);
		for (const a of ownedInResult) {
			for (const id of a.ids) {
				if (all) next.delete(id);
				else next.set(id, { card_id: a.card_id, printing_id: a.printing_id });
			}
		}
		selected = next;
	}

	/** Re-run the search after a bulk mutation, then drop the selection. */
	async function refresh() {
		await runSearch();
		selected = new Map();
	}

	async function bulkDelete() {
		const ids = [...selected.keys()];
		if (!ids.length || !confirm(`Delete ${ids.length} selected ${ids.length === 1 ? 'copy' : 'copies'}?`))
			return;
		busy = true;
		try {
			await api.bulkDelete(ids);
			await refresh();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	async function bulkAssign(field: 'binder_id' | 'deck_id', value: string) {
		const id = Number(value);
		if (!id) return;
		const ids = [...selected.keys()];
		busy = true;
		try {
			for (const copyId of ids) {
				await api.moveCopy(copyId, { [field]: id });
			}
			await refresh();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	// Set condition on every selected copy in one batch, then refresh once —
	// far quicker than opening each card modal in turn (dumpstar-4s8). The
	// updates are independent, so fire them together.
	async function bulkSetCondition(condition: string) {
		if (!condition) return;
		const ids = [...selected.keys()];
		if (!ids.length) return;
		busy = true;
		error = null;
		try {
			await Promise.all(ids.map((id) => api.updateCopy(id, { condition })));
			await refresh();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	async function bulkWishlist() {
		// One wish per distinct card — selecting two copies of a card wishes it
		// once. Read off the selection itself, not off the page's rows: a copy
		// selected three pages ago is still selected and still has to be wished.
		const seen = new Set<string>();
		const wishes = [...selected.values()].filter((r) =>
			seen.has(r.card_id) ? false : (seen.add(r.card_id), true)
		);
		busy = true;
		try {
			for (const r of wishes) {
				await api.addWish({ card_id: r.card_id, printing_id: r.printing_id });
			}
			selected = new Map();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Collection — Dumpstar</title></svelte:head>

<svelte:window
	onkeydown={(e) => {
		if (e.key === 'Escape') {
			menuOpen = false;
			columnsOpen = false;
		}
	}}
/>

<!--
  DD-style chrome: a sticky top bar with brand → home, the search
  pinned to the viewport, glyph view toggle, and Export/Select hidden
  behind a burger. The global nav is suppressed for this page by
  +layout.svelte.

  One row, not two. The controls either side of the search are all small
  and all rare — a view toggle, a menu, a count — and stacking them on
  their own line doubled the height of the only chrome that is always on
  screen. They ride the search's line and wrap under it on a narrow
  viewport, which is what Toolbar's `wrap` already does.

  `surface="panel"` rather than the translucent sticky fill: the search
  input's boundary has to clear 3:1 (WCAG 1.4.11) against whatever is
  behind it, and it manages 2.61:1 on the saturated blue. The panel is
  the quieter ground anyway — it sits a hair off the page colour instead
  of banding it.
-->
<Toolbar
	class="topbar"
	direction="column"
	align="stretch"
	gap="sm"
	wrap={false}
	sticky
	surface="panel"
>
	<Toolbar gap="md" wrap={false}>
		<a class="brand" href="/" aria-label="Home" title="Home">
			<span class="brandmark"><DeathStar size={26} /></span>
		</a>
		<div class="searchwrap">
			<input
				class="search"
				class:error={searchError !== null}
				data-testid="search-input"
				type="text"
				role="combobox"
				aria-expanded={acOpen}
				aria-controls="search-ac"
				autocomplete="off"
				placeholder={allCards ? 'Search all cards… (t:unit a:vigilance cost<=3)' : 'Search… (t:unit a:vigilance cost<=3)'}
				value={searchRaw}
				oninput={(e) => onSearch(e.currentTarget.value)}
				onkeydown={onSearchKeydown}
				onfocus={() => (acFocused = true)}
				onblur={() => setTimeout(() => (acFocused = false), 120)}
				bind:this={searchInput}
			/>
			{#if searchRaw}
				<button
					class="searchclear"
					type="button"
					aria-label="Clear search"
					title="Clear"
					onclick={() => {
						onSearch('');
						searchInput?.focus();
					}}
				>×</button>
			{/if}
			{#if acOpen}
				<ul class="acmenu" id="search-ac" data-testid="search-autocomplete" role="listbox">
					{#each suggestions as s, i (s.label)}
						<li>
							<button
								type="button"
								class="acitem"
								class:active={i === acIndex}
								role="option"
								aria-selected={i === acIndex}
								onmousedown={(e) => {
									e.preventDefault();
									acceptSuggestion(s);
								}}
							>
								<span class="ackey">{s.label}</span>
								{#if s.hint}<span class="achint">{s.hint}</span>{/if}
							</button>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
		<a class="helplink" data-testid="search-help-link" href="/search-help" title="Search syntax help"
			>?</a
		>
	</Toolbar>
	<Toolbar gap="md" wrap={false} class="controlsrow">
		{#if searchRows.length > 0}
			<div class="viewtoggle" role="group" aria-label="View">
				<button
					class:on={view === 'table'}
					data-testid="view-table"
					onclick={() => (view = 'table')}
					aria-label="Table view"
					title="Table"
				><svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><rect x="1" y="2" width="14" height="2" rx=".5" /><rect x="1" y="7" width="14" height="2" rx=".5" /><rect x="1" y="12" width="14" height="2" rx=".5" /></svg></button>
				<button
					class:on={view === 'grid'}
					data-testid="view-grid"
					onclick={() => (view = 'grid')}
					aria-label="Grid view"
					title="Grid"
				><svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><rect x="1" y="1" width="6" height="6" rx="1" /><rect x="9" y="1" width="6" height="6" rx="1" /><rect x="1" y="9" width="6" height="6" rx="1" /><rect x="9" y="9" width="6" height="6" rx="1" /></svg></button>
			</div>
			<div class="burgerWrap">
				<button
					class="burger"
					data-testid="more-menu"
					onclick={() => (menuOpen = !menuOpen)}
					aria-label="Menu"
					aria-expanded={menuOpen}
					title="More options"
				>⋮</button>
				{#if menuOpen}
					<div class="menu menuLeft" role="menu">
						<label class="menuItem" title="Search the full card catalog, not just your collection">
							<input
								type="checkbox"
								data-testid="all-cards-toggle"
								checked={allCards}
								onchange={toggleAllCards}
							/>
							All cards
						</label>
						<a class="menuItem" href="/api/export/csv" download onclick={closeMenu}>Export CSV (ManaBox)</a>
						<a class="menuItem" href="/api/export/collectr/singles.csv" download onclick={closeMenu}>Export cards (Collectr)</a>
						<a class="menuItem" href="/api/export/collectr/sealed.csv" download onclick={closeMenu}>Export sealed (Collectr)</a>
						<a class="menuItem" href="/api/export/json" download onclick={closeMenu}>Export JSON (full backup)</a>
						<button
							class="menuItem"
							onclick={() => {
								toggleSelectMode();
								closeMenu();
							}}
						>
							{selectMode ? 'Cancel select' : 'Select'}
						</button>
					</div>
				{/if}
			</div>
		{/if}
		<!-- The count speaks for the whole result set, and so does the money
		     beside it, because the endpoint answers both (pd-2g84). The money
		     is the value of the OWNED copies among those rows — the same
		     copies the chart it opens values — so it is a figure this line and
		     that chart can both defend, and it stays true of the whole result
		     no matter how much of it the client is holding. -->
		<button
			type="button"
			class="countline muted"
			data-testid="collection-count"
			onclick={() => (valueOpen = true)}
			title="Collection value over time"
		>
			{count(searchTotal)}
			cards{#if searchTotalValue !== null}, {money(searchTotalValue)}{/if}
		</button>
		{#if searchRows.length > 0 && view === 'table'}
			<div class="burgerWrap">
				<button
					class="colsbtn"
					data-testid="columns-button"
					onclick={() => (columnsOpen = !columnsOpen)}
					aria-expanded={columnsOpen}
					aria-label="Configure columns"
					title="Configure columns"
				><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1 1h4v4H1zm5 0h4v4H6zm5 0h4v4H11zM1 6h4v4H1zm5 0h4v4H6zm5 0h4v4H11zM1 11h4v4H1zm5 0h4v4H6zm5 0h4v4H11z" /></svg></button>
				{#if columnsOpen}
					<div class="menu" role="group" aria-label="Columns" data-testid="columns-menu">
						{#each COLUMNS as c (c.key)}
							{#if c.key !== 'adj' || adjustedPresent}
								<label class="menuItem">
									<input
										type="checkbox"
										data-testid="column-{c.key}"
										checked={!hiddenCols.includes(c.key)}
										onchange={() => toggleColumn(c.key)}
									/>
									{c.label}
								</label>
							{/if}
						{/each}
					</div>
				{/if}
			</div>
		{/if}
	</Toolbar>
	{#if searchError}
		<div class="searcherr" data-testid="search-error" role="alert">
			<span class="errmsg">{searchError.message}</span>
			<span class="errpos">position {searchError.position}</span>
		</div>
	{/if}
</Toolbar>

{#if menuOpen || columnsOpen}
	<div
		class="menuBackdrop"
		role="presentation"
		onclick={() => {
			closeMenu();
			columnsOpen = false;
		}}
	></div>
{/if}

<!-- The layout runs this route flush to the viewport edge so the bar above
     can pin edge to edge; everything below it gets its gutter back here.
     Card art needs a margin as much as it needs a gap. -->
<div class="content">
{#if loading}
	<p class="muted">Loading…</p>
{:else if error}
	<p class="error">Failed to load collection: {error}</p>
{:else}

	{#if selectMode && selected.size > 0}
		<div class="bulkbar">
			<span class="count">{selected.size} selected</span>
			<button disabled={busy} onclick={bulkDelete}>Delete</button>
			<select
				disabled={busy}
				onchange={(e) => {
					bulkSetCondition(e.currentTarget.value);
					e.currentTarget.selectedIndex = 0;
				}}
			>
				<option value="">Set condition…</option>
				{#each CONDITIONS as c (c)}<option value={c}>{c}</option>{/each}
			</select>
			<select
				disabled={busy || binders.length === 0}
				onchange={(e) => {
					bulkAssign('binder_id', e.currentTarget.value);
					e.currentTarget.selectedIndex = 0;
				}}
			>
				<option value="">Assign to binder…</option>
				{#each binders as b (b.id)}<option value={b.id}>{b.name}</option>{/each}
			</select>
			<select
				disabled={busy || decks.length === 0}
				onchange={(e) => {
					bulkAssign('deck_id', e.currentTarget.value);
					e.currentTarget.selectedIndex = 0;
				}}
			>
				<option value="">Assign to deck…</option>
				{#each decks as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
			</select>
			<button disabled={busy} onclick={bulkWishlist}>Add to wishlist</button>
		</div>
	{/if}

	{#if searchRows.length === 0}
		{#if searchError}
			<EmptyState
				title="That query didn't parse."
				description="Fix the query above to see results — the message under the box points at the character that stopped it."
			/>
		{:else if query}
			<EmptyState
				title="No cards match “{query}”."
				description={allCards
					? 'Nothing in the whole catalog matches. Check the spelling, or narrow one keyword at a time.'
					: 'Nothing you own matches. Turn on “All cards” to search the catalog instead of your collection.'}
			>
				{#snippet action()}
					<Button variant="ghost" href="/search-help">Search syntax</Button>
				{/snippet}
			</EmptyState>
		{:else if allCards}
			<EmptyState
				title="No cards in the catalog."
				description="The shared catalog has no cards in it, so there is nothing to search."
			/>
		{:else}
			<EmptyState
				title="Your collection is empty."
				description="Add cards from a set's binder view — click a slot and that printing is registered as a copy you own. Or turn on “All cards” to browse the catalog first."
			>
				{#snippet action()}
					<Button href="/browse">Browse sets</Button>
				{/snippet}
			</EmptyState>
		{/if}
	{:else if view === 'grid'}
		<!-- Grid lacks the table's sortable column headers, so it gets a
		     row of per-field buttons. Click an inactive button to switch
		     to that field (with a sensible default direction); click the
		     active one to toggle asc/desc. State is shared with the
		     table view (sortKey/sortDir) so flipping the view never
		     loses your sort.

		     The buttons share a min-width so the row reads as one control
		     rather than eight ragged pills, and the active one is a wash
		     with an accent edge — a solid crimson slab beside 4,763 pieces
		     of card art was the loudest thing on the page. -->
		<div class="gridsort" role="group" aria-label="Sort">
			<span class="gridsortlabel">Sort</span>
			{#snippet sortBtn(key: string, label: string)}
				<button
					class="sortbtn"
					class:active={sortKey === key}
					aria-pressed={sortKey === key}
					onclick={() => sortBy(key)}
				>
					{label}
					{#if sortKey === key}
						<span class="caret">{sortDir === 'asc' ? '▲' : '▼'}</span>
					{/if}
				</button>
			{/snippet}
			{@render sortBtn('name', 'Name')}
			{@render sortBtn('type', 'Type')}
			{@render sortBtn('cost', 'Cost')}
			{@render sortBtn('rarity', 'Rarity')}
			{@render sortBtn('set', 'Set')}
			{@render sortBtn('number', '#')}
			{@render sortBtn('price', 'Price')}
		</div>
		<!-- Sections are runs of `displayRows`, so tile order is exactly the
		     server's sort order — owned and unowned printings still
		     interleave.

		     Only the blocks under the viewport are here; the two spacers carry
		     the height of everything else, so the grid is as tall as all
		     56,635 tiles would make it while holding a few dozen of them
		     (pd-7z4o). Each spacer spans the grid, which is also what makes a
		     tile row start at column one after it. -->
		<div class="cardgrid" data-testid="collection-grid" bind:this={listEl}>
			{#if gridWindow.padTop > 0}
				<div class="vspace" style="height: {gridWindow.padTop}px" aria-hidden="true"></div>
			{/if}
			{#each gridVisible as { i, block } (i)}
				{#if block.kind === 'header'}
					<div class="groupheader">
						<SectionHeader
							size="sm"
							title={block.label}
							meta="{count(block.count)} {block.count === 1 ? 'card' : 'cards'}"
							divider
						/>
					</div>
				{:else}
					{#each displayRows.slice(block.from, block.to) as a (a.key)}
						{#if a.ids.length === 0}
							<button
								class="cardtile missing"
								title="{a.name} · {a.set_code.toUpperCase()} #{a.number} · click to add"
								onclick={() => (selectedCard = { set: a.set_code, number: a.number })}
							>
								{#if a.image_url}
									<img src={a.image_url} alt={a.name} loading="lazy" />
								{:else}
									<div class="tilenoart">{a.name}</div>
								{/if}
							</button>
						{:else}
							<button
								class="cardtile"
								class:picked={selectMode && groupChecked(a.ids)}
								class:foil={isFoil(a.finish)}
								title="{a.name} · {printingLabel(a.variant, a.finish)}{a.qty > 1 ? ` ×${a.qty}` : ''}"
								onclick={() => openGroup(a)}
							>
								{#if a.image_url}
									<img src={a.image_url} alt={a.name} loading="lazy" />
								{:else}
									<div class="tilenoart">{a.name}</div>
								{/if}
								{#if a.qty > 1}<span class="qtybadge">×{a.qty}</span>{/if}
								{#if selectMode && groupChecked(a.ids)}<span class="tick">✓</span>{/if}
							</button>
						{/if}
					{/each}
				{/if}
			{/each}
			{#if gridWindow.padBottom > 0}
				<div class="vspace" style="height: {gridWindow.padBottom}px" aria-hidden="true"></div>
			{/if}
		</div>
	{:else}
		{#snippet sortable(key: string, label: string, extra: string, title?: string)}
			<th class="sortable {extra}" {title} onclick={() => sortBy(key)}>
				{label}
				{#if sortKey === key}
					<span class="caret">{sortDir === 'asc' ? '▲' : '▼'}</span>
				{/if}
			</th>
		{/snippet}
		<div class="tableScroll">
		<table class="dd" class:selecting={selectMode} style={colVars}>
			<!-- Every column's width is DECLARED here, and `table-layout: fixed`
			     (below) makes the browser believe it. It cannot be derived from
			     content, because this table renders a virtual WINDOW of the
			     result (pd-7z4o): auto layout sizes each column from the rows
			     currently in the DOM, the scroller replaces those rows on every
			     scroll tick, and so every column re-measured and jumped as the
			     reader moved (pd-hije — measured at 78px on the Name column,
			     33px on `#`, 23px on each price column over a 50k-row catalog).
			     A window cannot be sized from its own contents; the numbers here
			     are what auto layout settled on across the WHOLE result. -->
			<colgroup>
				{#if selectMode}<col class="c-sel" />{/if}
				{#if showCol('qty')}<col class="c-qty" />{/if}
				<col class="c-name" />
				{#if showCol('type')}<col class="c-type" />{/if}
				{#if showCol('aspects')}<col class="c-aspects" />{/if}
				{#if showCol('cost')}<col class="c-cost" />{/if}
				{#if showCol('stats')}<col class="c-stats" />{/if}
				{#if showCol('rarity')}<col class="c-rarity" />{/if}
				{#if showCol('set')}<col class="c-set" />{/if}
				{#if showCol('number')}<col class="c-number" />{/if}
				{#if showCol('nm')}<col class="c-nm" />{/if}
				{#if showCol('adj')}<col class="c-adj" />{/if}
			</colgroup>
			<thead>
				<tr>
					{#if selectMode}
						<th class="cbcol">
							<input type="checkbox" checked={tableAllSelected} onchange={toggleTableAll} />
						</th>
					{/if}
					{#if showCol('qty')}{@render sortable('qty', 'Qty', 'num qty')}{/if}
					{@render sortable('name', 'Name', '')}
					{#if showCol('type')}{@render sortable('type', 'Type', '')}{/if}
					{#if showCol('aspects')}<th>Aspects</th>{/if}
					{#if showCol('cost')}{@render sortable('cost', 'Cost', 'num')}{/if}
					{#if showCol('stats')}<th class="center" title="Power / HP">Pwr/HP</th>{/if}
					{#if showCol('rarity')}{@render sortable('rarity', 'Rarity', 'center')}{/if}
					{#if showCol('set')}{@render sortable('set', 'Set', 'center')}{/if}
					{#if showCol('number')}{@render sortable('number', '#', 'num')}{/if}
					{#if showCol('nm')}{@render sortable('price', 'Price', 'num', 'Near Mint market price (per copy)')}{/if}
					<!-- The one price column that does not sort. Adj. is the
					     copy's condition-adjusted price — the collection's
					     market price joined to the tenant's own conditions
					     across the ATTACH boundary, which no index can span, so
					     ordering 56k rows by it had to sort them all before
					     LIMIT could discard any (pd-tjym). Drawing it for the
					     rows under the viewport costs nothing, so it still
					     draws; it is a plain header rather than a dead
					     `.sortable` one, because a header that looks clickable
					     and isn't is worse than one that doesn't. -->
					{#if showCol('adj')}<th class="num" title="Condition-adjusted price (per copy)">Adjusted</th>{/if}
				</tr>
			</thead>
			<!-- Only the rows under the viewport, with a spacer row either side
			     carrying the height of the rest — same contract as the grid
			     above (pd-7z4o). -->
			<tbody bind:this={listEl}>
				{#if tableWindow.padTop > 0}
					<tr class="vspace" aria-hidden="true">
						<td colspan={tableCols} style="height: {tableWindow.padTop}px"></td>
					</tr>
				{/if}
				{#each tableVisible as a (a.key)}
					{#if a.ids.length === 0}
						<tr class="missing">
							{#if selectMode}<td class="cbcol"></td>{/if}
							{#if showCol('qty')}<td class="num qty"><span class="pricedash">—</span></td>{/if}
							<td class="namecol" title="Open card" onclick={(e) => openCardCell(e, a)}>
								<!-- Same shape as the owned row's name cell below, tags
								     aside: the `.namebody` wrapper is what keeps the name
								     to one line, and a row that skipped it was 3px taller
								     than its neighbours (pd-7z4o). -->
								<div class="namecell">
									{#if a.image_url}
										<img class="cardthumb" src={a.image_url} alt="" loading="lazy" />
									{/if}
									<span class="namebody"><span class="cardname">{a.name}</span></span>
								</div>
							</td>
							{#if showCol('type')}<td class="fac" title="Find all {a.card_type ?? ''} cards" onclick={(e) => facetCell(e, 'type', a.card_type)}>{a.card_type ?? ''}</td>{/if}
							{#if showCol('aspects')}<td class="fac aspects" title="Find cards of this aspect" onclick={(e) => facetCell(e, 'aspect', sortAspects(jsonList(a.aspects))[0])}>{#each sortAspects(jsonList(a.aspects)) as n (n)}<span class="fac-aspect" data-aspect={n} title="Find all {n} cards" onclick={(e) => facetCell(e, 'aspect', n)}><AspectIcon name={n} /></span>{/each}</td>{/if}
							{#if showCol('cost')}<td class="center"><CostMark cost={a.cost} /></td>{/if}
							{#if showCol('stats')}<td class="center stats"><StatFrame kind="power" value={a.power} /> <StatFrame kind="hp" value={a.hp} /></td>{/if}
							{#if showCol('rarity')}<td class="center fac" title="Find all {a.rarity ?? ''} cards" onclick={(e) => facetCell(e, 'rarity', a.rarity)}><RarityMark name={a.rarity} /></td>{/if}
							{#if showCol('set')}<td class="center fac" title="Find all cards in this set" onclick={(e) => facetCell(e, 'set', a.set_code)}>
								<span title={a.set_name}><SetLogo code={a.set_code} name={a.set_name} />{a.set_code.toUpperCase()}</span>
							</td>{/if}
							{#if showCol('number')}<td class="num">{a.number}</td>{/if}
							{#if showCol('nm')}<td class="num">
								{#if a.nm_unit != null}
									<span class="pricebox">{money(a.nm_unit)}</span>
								{:else}
									<span class="pricedash">—</span>
								{/if}
							</td>{/if}
							{#if showCol('adj')}<td class="num">
								{#if a.market_unit != null}
									<span class="pricebox">{money(a.market_unit)}</span>
								{:else}
									<span class="pricedash">—</span>
								{/if}
							</td>{/if}
						</tr>
					{:else}
					<tr class:picked={selectMode && groupChecked(a.ids)} onclick={() => { if (selectMode) toggleGroup(a); }}>
						{#if selectMode}
							<td class="cbcol" onclick={(e) => e.stopPropagation()}>
								<input
									type="checkbox"
									checked={groupChecked(a.ids)}
									onchange={() => toggleGroup(a)}
								/>
							</td>
						{/if}
						{#if showCol('qty')}<td class="num qty">{a.qty}</td>{/if}
						<td class="namecol" title="Open card" onclick={(e) => openCardCell(e, a)}>
							<div class="namecell">
								{#if a.image_url}
									<span class="thumbwrap" class:foil={isFoil(a.finish)}>
										<img class="cardthumb" src={a.image_url} alt="" loading="lazy" />
									</span>
								{/if}
								<span class="namebody"
									><span class="cardname">{a.name}</span>{#if printingTag(a.variant, a.finish, a.set_name)}<span
											class="tag vtag"
											title={printingLabel(a.variant, a.finish)}>{printingTag(a.variant, a.finish, a.set_name)}</span
										>{/if}{#if statusBadge(a.status)}<span
											class="tag stag t-{a.status}"
											title={a.status}>{statusBadge(a.status)}</span
										>{/if}</span
								>
							</div>
						</td>
						{#if showCol('type')}<td class="fac" title="Find all {a.card_type ?? ''} cards" onclick={(e) => facetCell(e, 'type', a.card_type)}>{a.card_type ?? ''}</td>{/if}
						{#if showCol('aspects')}<td class="fac aspects" title="Find cards of this aspect" onclick={(e) => facetCell(e, 'aspect', sortAspects(jsonList(a.aspects))[0])}>{#each sortAspects(jsonList(a.aspects)) as n (n)}<span class="fac-aspect" data-aspect={n} title="Find all {n} cards" onclick={(e) => facetCell(e, 'aspect', n)}><AspectIcon name={n} /></span>{/each}</td>{/if}
						{#if showCol('cost')}<td class="center"><CostMark cost={a.cost} /></td>{/if}
						{#if showCol('stats')}<td class="center stats"><StatFrame kind="power" value={a.power} /> <StatFrame kind="hp" value={a.hp} /></td>{/if}
						{#if showCol('rarity')}<td class="center fac" title="Find all {a.rarity ?? ''} cards" onclick={(e) => facetCell(e, 'rarity', a.rarity)}><RarityMark name={a.rarity} /></td>{/if}
						{#if showCol('set')}<td class="center fac" title="Find all cards in this set" onclick={(e) => facetCell(e, 'set', a.set_code)}>
							<span title={a.set_name}><SetLogo code={a.set_code} name={a.set_name} />{a.set_code.toUpperCase()}</span>
						</td>{/if}
						{#if showCol('number')}<td class="num">{a.number}</td>{/if}
						{#if showCol('nm')}<td class="num">
							{#if a.nm_unit != null}
								<span class="pricebox">{money(a.nm_unit)}</span>
							{:else}
								<span class="pricedash">—</span>
							{/if}
						</td>{/if}
						{#if showCol('adj')}<td class="num">
							{#if a.market_unit != null}
								<span class="pricebox">{money(a.market_unit)}</span>
							{:else}
								<span class="pricedash">—</span>
							{/if}
						</td>{/if}
					</tr>
					{/if}
				{/each}
				{#if tableWindow.padBottom > 0}
					<tr class="vspace" aria-hidden="true">
						<td colspan={tableCols} style="height: {tableWindow.padBottom}px"></td>
					</tr>
				{/if}
			</tbody>
		</table>
		</div>
	{/if}
{/if}
</div>

{#if selectedCard}
	<CardModal
		setCode={selectedCard.set}
		number={selectedCard.number}
		onClose={closeCard}
		onMutate={() => (cardDirty = true)}
	/>
{/if}

{#if valueOpen}
	<ValueHistoryModal onClose={() => (valueOpen = false)} />
{/if}

<style>
	.muted {
		color: var(--color-text-subtle);
	}
	.error {
		color: var(--color-danger-text);
	}

	/* --- DD-style top chrome ------------------------------------------- */

	/* The band itself is Toolbar `sticky surface="panel"` — position, fill,
	   rule and padding all belong to the primitive. What is left here is
	   what sits inside it. */

	/* Everything below the pinned bar. `main.flush` strips the layout's
	   padding for this route so the bar can pin edge to edge; the gutter
	   comes back here, where the content is. */
	.content {
		--gutter-x: var(--space-6);
		--gutter-top: var(--space-5);
		padding: var(--gutter-top) var(--gutter-x) var(--space-10);
	}
	.brand {
		display: inline-flex;
		align-items: center;
		text-decoration: none;
		flex-shrink: 0;
	}
	.brandmark {
		width: 26px;
		height: 26px;
		display: block;
	}
	.brand:hover .brandmark {
		filter: brightness(1.2);
	}
	/* The input inherits the row's flex slot via its wrapper, so the
	   wrapper carries the flex sizing rather than the input itself. The
	   basis is what makes the single row wrap gracefully: below ~44rem of
	   free space the search claims the first line on its own and the small
	   controls drop under it. The cap stops it from stretching into a
	   1920-wide trough that no query ever fills. */
	.searchwrap {
		flex: 1;
		min-width: 0;
		position: relative;
		display: flex;
		align-items: center;
	}
	.search {
		flex: 1;
		min-width: 0;
		/* Extra right padding leaves room for the clear button without
		   it overlapping typed text. */
		padding: var(--space-3) var(--space-8) var(--space-3) var(--space-4);
		background: var(--color-control-surface);
		border: 1px solid var(--color-control-border);
		border-radius: var(--radius-md);
		color: var(--color-control-text);
		font: inherit;
	}
	/* An invalid control reddens on the danger ramp, not the brand one —
	   the same answer the Field primitive gives for `error`. Crimson here
	   was the "delete reads as brand" muddle the two ramps exist to end. */
	.search.error {
		border-color: var(--color-danger);
	}
	.searcherr {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		font-size: var(--text-sm);
		color: var(--color-danger-text);
	}
	.errpos {
		color: var(--color-text-subtle);
	}
	/* deckdumpster's .syntax-help-btn: a 38px square, accent border and
	   accent '?', filling with the accent on hover. */
	.helplink {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2.375rem;
		height: 2.375rem;
		flex-shrink: 0;
		background: var(--color-control-surface);
		border: 1px solid var(--color-accent);
		border-radius: var(--radius-md);
		color: var(--color-accent);
		text-decoration: none;
		font-weight: var(--weight-semibold);
	}
	.helplink:hover {
		background: var(--color-accent);
		color: var(--color-on-accent);
	}
	.acmenu {
		position: absolute;
		top: calc(100% + 2px);
		left: 0;
		right: 0;
		z-index: 60;
		margin: var(--space-0);
		padding: var(--space-1);
		list-style: none;
		background: var(--color-surface-overlay);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-lg);
		max-height: 16rem;
		overflow-y: auto;
	}
	.acitem {
		display: flex;
		align-items: baseline;
		gap: 0.6rem;
		width: 100%;
		text-align: left;
		background: none;
		border: none;
		border-radius: var(--radius-sm);
		padding: 0.35rem 0.5rem;
		color: var(--color-text);
		font: inherit;
		cursor: pointer;
	}
	.acitem.active,
	.acitem:hover {
		background: var(--color-surface-hover);
	}
	.ackey {
		font-family: var(--font-mono);
		color: var(--color-warning-text);
		white-space: nowrap;
	}
	.achint {
		color: var(--color-text-subtle);
		font-size: 0.82rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.searchclear {
		position: absolute;
		right: 0.4rem;
		top: 50%;
		transform: translateY(-50%);
		width: 1.4rem;
		height: 1.4rem;
		padding: var(--space-0);
		background: none;
		border: none;
		color: var(--color-text-subtle);
		font-size: var(--text-xl);
		line-height: 1;
		border-radius: var(--radius-round);
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}
	.searchclear:hover {
		color: var(--color-text-accent);
		background: var(--color-surface-selected);
	}
	.countline {
		margin: var(--space-0) var(--space-0) var(--space-0) auto;
		white-space: nowrap;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		font-size: var(--text-md);
		background: none;
		border: none;
		padding: var(--space-0);
		font-family: inherit;
		cursor: pointer;
	}
	.countline:hover {
		color: var(--color-text);
		text-decoration: underline;
	}
	/* The table runs edge to edge: it cancels `.content`'s gutter on the
	   sides and under the pinned bar, by the gutter the viewport actually has
	   (cancelling the desktop one pushed the table off the left edge on a
	   phone, clipping the first heading). A bulk bar above it keeps the top gap. */
	.tableScroll {
		overflow-x: auto;
		margin: calc(-1 * var(--gutter-top)) calc(-1 * var(--gutter-x)) 0;
	}
	.bulkbar ~ .tableScroll {
		margin-top: 0;
	}
	.viewtoggle {
		display: flex;
	}
	.viewtoggle button {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		box-sizing: border-box;
		height: 2.375rem;
		padding: 0 var(--space-3);
		background: var(--color-control-surface);
		border: 1px solid var(--color-control-border);
		color: var(--color-control-text);
		line-height: 1;
		cursor: pointer;
	}
	.viewtoggle button svg {
		display: block;
	}
	.viewtoggle button:first-child {
		border-radius: var(--radius-md) 0 0 var(--radius-md);
	}
	.viewtoggle button:last-child {
		border-radius: 0 var(--radius-md) var(--radius-md) 0;
		margin-left: calc(-1 * var(--space-px));
	}
	.viewtoggle button.on {
		position: relative;
		z-index: 1;
		background: var(--color-info-surface);
		border-color: var(--color-accent);
	}
	.burger,
	.colsbtn {
		background: var(--color-control-surface);
		border: 1px solid var(--color-control-border);
		color: var(--color-text-subtle);
		font-size: 1.3rem;
		line-height: 1;
		box-sizing: border-box;
		height: 2.375rem;
		padding: 0 var(--space-3);
		cursor: pointer;
		border-radius: var(--radius-md);
	}
	.burger:hover,
	.colsbtn:hover {
		color: var(--color-accent);
		border-color: var(--color-accent);
	}
	.colsbtn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2.375rem;
		padding: 0;
		flex-shrink: 0;
	}
	.colsbtn svg {
		width: 1rem;
		height: 1rem;
		fill: currentColor;
	}

	.menuBackdrop {
		position: fixed;
		inset: 0;
		z-index: 49;
	}
	/* Burger wrapper anchors the dropdown to the button itself — no
	   matter how tall the topbar grows, the menu lands just below the
	   ⋯ and never overlays it. */
	.burgerWrap {
		position: relative;
		display: inline-flex;
	}
	.menu {
		position: absolute;
		top: calc(100% + 4px);
		/* Anchored to the ⋯'s right edge, not its left: the button now rides
		   the right-hand end of a single-row bar, and a left-anchored menu
		   would hang off the viewport on a narrow one. */
		right: 0;
		z-index: 60;
		display: flex;
		flex-direction: column;
		/* Wide enough for "Export JSON (full backup)" on one line — the
		   labels are the width, not a round number. */
		min-width: 15rem;
		background: var(--color-surface-overlay);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		padding: 0.3rem;
		box-shadow: var(--shadow-md);
	}
	.menuLeft {
		right: auto;
		left: 0;
	}
	.menuItem {
		background: none;
		border: none;
		color: var(--color-text);
		text-align: left;
		padding: var(--space-2) var(--space-3);
		font: inherit;
		font-size: var(--text-lg);
		white-space: nowrap;
		border-radius: var(--radius-sm);
		cursor: pointer;
		text-decoration: none;
		display: block;
	}
	.menuItem:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-accent);
	}

	/* --- Grid view ------------------------------------------------------ */

	.gridsort {
		display: flex;
		gap: var(--space-2);
		align-items: center;
		flex-wrap: wrap;
		margin: var(--space-0) var(--space-0) var(--space-5);
		font-size: var(--text-md);
	}
	.gridsortlabel {
		font-size: var(--text-xs);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--color-text-subtle);
		margin-right: var(--space-1);
	}
	.sortbtn {
		/* One width for every pill. The labels run from "#" to "Rarity",
		   and letting each shrink to its text turned a row of peers into a
		   ragged line whose widths encoded nothing. */
		min-width: 5.5rem;
		justify-content: center;
		background: none;
		border: 1px solid var(--color-border-subtle);
		color: var(--color-text-subtle);
		border-radius: var(--radius-pill);
		padding: var(--space-1) var(--space-3);
		font: inherit;
		font-size: var(--text-md);
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		transition:
			background-color var(--dur-fast) var(--ease-standard),
			border-color var(--dur-fast) var(--ease-standard),
			color var(--dur-fast) var(--ease-standard);
	}
	.sortbtn:hover {
		border-color: var(--color-border-accent);
		color: var(--color-text);
	}
	.sortbtn:focus-visible {
		outline: none;
		box-shadow: var(--shadow-focus);
	}
	/* The active pill is a wash and an edge, not a slab. Solid crimson next
	   to card art read as the loudest element on a page whose whole job is
	   to show the art; the accent still marks the pill, at a weight that
	   loses to the art. */
	.sortbtn.active {
		background: var(--color-surface-selected);
		border-color: var(--color-border-accent);
		color: var(--color-text-accent);
	}
	.sortbtn .caret {
		/* Inherit rather than take the global .caret accent: on the active
		   pill the label is already accent-coloured, and a second tone
		   inside one control is noise. */
		color: inherit;
		font-size: 0.65rem;
		opacity: 0.9;
	}
	/* Fewer cards per row, bigger art, real gutters. The tile floor steps up
	   with the viewport rather than staying at a phone-sized 130px on a
	   1920 monitor, where it produced fourteen columns of thumbnails. */
	.cardgrid {
		--tile-min: 150px;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(var(--tile-min), 1fr));
		gap: var(--space-4);
		margin-top: var(--space-0);
	}
	@media (min-width: 1200px) {
		.cardgrid {
			--tile-min: 176px;
		}
	}
	@media (min-width: 1700px) {
		.cardgrid {
			--tile-min: 204px;
		}
	}
	/* A section label spans the grid. SectionHeader owns its own type and
	   rule; this only places it. */
	.groupheader {
		grid-column: 1 / -1;
	}
	/* The virtual scroller's spacers (pd-7z4o) — empty boxes carrying the
	   height of the tiles that are not in the document, so the page is as tall
	   as the whole result. Spanning the grid is not only cosmetic: a full-width
	   item ends the row, which is what puts the first rendered tile back at
	   column one. */
	.vspace {
		grid-column: 1 / -1;
	}
	.cardtile {
		position: relative;
		padding: var(--space-0);
		background: none;
		border: 2px solid transparent;
		border-radius: var(--radius-lg);
		cursor: pointer;
	}
	.cardtile img {
		width: 100%;
		display: block;
		aspect-ratio: 5 / 7;
		object-fit: contain;
		background: var(--color-surface-well);
		border-radius: var(--radius-md);
	}
	.cardtile.picked {
		border-color: var(--color-border-accent);
	}
	/* Catalog rows the user doesn't own — mirror the .missing treatment
	   the browse view uses for empty binder slots. */
	.cardtile.missing img,
	.cardtile.missing .tilenoart {
		filter: grayscale(0.9) brightness(0.62);
	}
	.cardtile.missing {
		opacity: 0.82;
	}
	/* Foil treatment for Foil-finish
	   printings. Two pseudo-elements stacked over the card image:
	   ::before paints a rainbow tint via mix-blend-mode, ::after
	   animates a diagonal sheen across the surface. Pointer-events
	   none so clicks still hit the underlying button. The .cardtile
	   already has rounded corners + overflow handled by its img;
	   give the foil overlays the same border-radius and clip them
	   to the tile.

	   Both treatments are theme composites (--gradient-foil-*): a foil is
	   a card-game idiom, not a colour, so it belongs to the theme whole
	   rather than being reassembled from seven stops per component. */
	.cardtile.foil {
		overflow: hidden;
	}
	.cardtile.foil::before,
	.thumbwrap.foil::before {
		content: '';
		position: absolute;
		inset: 0;
		border-radius: inherit;
		background: var(--gradient-foil-spectrum);
		mix-blend-mode: color;
		pointer-events: none;
		z-index: 1;
	}
	.cardtile.foil::after,
	.thumbwrap.foil::after {
		content: '';
		position: absolute;
		inset: 0;
		border-radius: inherit;
		background: var(--gradient-foil-streak) 100% 100% / 240% 240%;
		pointer-events: none;
		z-index: 2;
		animation: foil-streak 3.2s ease-in-out infinite;
	}
	@keyframes foil-streak {
		0% {
			background-position: 100% 100%;
		}
		15%,
		100% {
			background-position: 0 0;
		}
	}
	/* Wrapper around the small table-row thumbnail so foil pseudos
	   have somewhere to attach (an <img> can't host pseudo-elements). */
	.thumbwrap {
		position: relative;
		display: inline-block;
		flex-shrink: 0;
		border-radius: var(--radius-xs);
		overflow: hidden;
	}
	/* Quantity badge in the corner of an aggregated grid tile. */
	.qtybadge {
		position: absolute;
		top: 4px;
		right: 4px;
		background: var(--color-surface-sticky);
		color: var(--color-text);
		font-size: var(--text-xs);
		font-weight: var(--weight-bold);
		padding: 0.1rem 0.4rem;
		border-radius: var(--radius-pill);
		z-index: 3;
		pointer-events: none;
	}
	.tilenoart {
		aspect-ratio: 5 / 7;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-surface-panel);
		border-radius: var(--radius-md);
		color: var(--color-text-subtle);
		font-size: var(--text-sm);
		padding: var(--space-2);
		text-align: center;
	}
	.tick {
		position: absolute;
		top: 5px;
		right: 5px;
		width: 22px;
		height: 22px;
		border-radius: var(--radius-round);
		background: var(--color-accent);
		color: var(--color-on-accent);
		font-size: var(--text-sm);
		display: flex;
		align-items: center;
		justify-content: center;
	}

	/* --- Multi-select bulk bar ---------------------------------------- */

	.bulkbar {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
		margin: var(--space-0) var(--space-0) var(--space-4);
		padding: var(--space-3) var(--space-4);
		background: var(--color-surface-panel);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
	}
	.bulkbar .count {
		font-size: var(--text-md);
		color: var(--color-text-accent);
		font-weight: var(--weight-semibold);
	}
	.bulkbar button,
	.bulkbar select {
		background: var(--color-info-surface);
		border: none;
		border-radius: var(--radius-md);
		color: var(--color-text);
		padding: 0.35rem 0.7rem;
		font-size: var(--text-sm);
		cursor: pointer;
	}
	.bulkbar button:hover:not(:disabled),
	.bulkbar select:hover:not(:disabled) {
		background: var(--color-accent);
		/* Dark ink on the crimson fill, as Button's `primary` does. */
		color: var(--color-on-accent);
	}
	.bulkbar button:disabled,
	.bulkbar select:disabled {
		opacity: 0.5;
		cursor: default;
	}

	/* --- Table view (DeckDumpster-style) ------------------------------ */

	table.dd {
		/* One width per column, spent by the <colgroup> above and by the
		   `min-width` below — declared in one place so the two can never
		   disagree. The figures are what `table-layout: auto` settled each
		   column on across a whole 50k-row catalog, rounded up a step. Where a
		   column can want more than its share — Name and Type, the two that
		   carry prose — the cell ellipsises; both of them already did that
		   before, at a width that moved. */
		--col-sel: 2.25rem;
		--col-qty: 2.5rem;
		/* Card type ("Leader", "Upgrade"), the aspect icons (a card carries
		   at most two: 2 x 1.25em icon at this table's 0.9rem, plus the
		   cell's 0.6rem padding each side), resource cost, and power/HP. */
		--col-type: 5.5rem;
		--aspect-n: 2;
		--col-aspects: calc(var(--aspect-n) * 1.25em + 1.2rem + 1px);
		--col-cost: 3.5rem;
		--col-stats: 5.5rem;
		--col-rarity: 5.5rem;
		--col-set: 4.25rem;
		--col-number: 3.75rem;
		--col-nm: 4.75rem;
		--col-adj: 5.5rem;
		/* Not a column width — the narrowest the flex column may be squeezed
		   to before the table stops shrinking and `.tableScroll` scrolls
		   instead. Measured: the thumbnail (110px) plus its gap plus enough
		   name to read, which is what auto layout already bottomed out at. */
		--col-name-min: 13rem;
		/* What the select column contributes to that floor: nothing until there
		   IS one. Indirected rather than repeated in the `min-width` below, so
		   the narrow-viewport block can retune `--col-sel` alone and both modes
		   follow. Without it, turning selection on takes the checkbox column's
		   width out of the name column instead of out of the table. */
		--col-sel-used: 0rem;

		/* Span the full container so the header underline reaches the right
		   edge on wide viewports; the Name column absorbs the leftover. */
		width: 100%;
		/* Below this the table stops absorbing and the container scrolls —
		   the same behaviour auto layout already had at 768 and narrower,
		   except that it now happens at ONE width instead of a width that
		   changed with whichever rows were rendered. */
		min-width: calc(
			var(--col-sel-used) + var(--col-qty) + var(--col-name-min) + var(--col-type) +
				var(--col-aspects) + var(--col-cost) + var(--col-stats) + var(--col-rarity) + var(--col-set) +
				var(--col-number) + var(--col-nm) + var(--col-adj)
		);
		/* The point of the exercise: widths come from the declarations above,
		   not from the handful of rows the virtual scroller happens to have in
		   the DOM. See the <colgroup> comment in the markup. */
		table-layout: fixed;
		border-collapse: collapse;
		font-size: 0.9rem;
		margin-top: var(--space-0);
	}
	/* `col` carries width and nothing else — every other property on a column
	   is ignored, so these stay one line each. */
	table.dd.selecting {
		--col-sel-used: var(--col-sel);
	}
	col.c-sel {
		width: var(--col-sel);
	}
	col.c-qty {
		width: var(--col-qty);
	}
	/* The flex column: the only `auto` one, so under fixed layout it takes
	   whatever the declared columns leave. Today it's the Name column. */
	col.c-name {
		width: auto;
	}
	col.c-type {
		width: var(--col-type);
	}
	col.c-aspects {
		width: var(--col-aspects);
	}
	table.dd td.aspects {
		white-space: nowrap;
	}
	col.c-cost {
		width: var(--col-cost);
	}
	col.c-stats {
		width: var(--col-stats);
	}
	col.c-rarity {
		width: var(--col-rarity);
	}
	col.c-set {
		width: var(--col-set);
	}
	col.c-number {
		width: var(--col-number);
	}
	col.c-nm {
		width: var(--col-nm);
	}
	col.c-adj {
		width: var(--col-adj);
	}
	table.dd th,
	table.dd td {
		padding: 0.35rem 0.6rem;
		border-bottom: 1px solid var(--color-border);
		vertical-align: middle;
	}
	/* One row height for every row, because the virtual scroller's geometry is
	   arithmetic over a single one (pd-7z4o) — a row 3px out of step is a
	   scrollbar 3px × 56,635 rows out of step. The thumbnail is what sets the
	   height, so `--row-content` is the thumbnail, and nothing in a cell may
	   grow past it: a long aspects list is clipped, exactly as the name
	   cell next to it ellipsises. */
	table.dd {
		--row-content: 36px;
	}
	table.dd tbody td > * {
		max-height: var(--row-content);
		overflow: hidden;
	}
	/* An inline image inside the foil wrapper sits on a baseline, and the
	   descender space under it made the wrapper 3px taller than the image —
	   the one row-height difference that was not the name wrapping. */
	.cardthumb {
		display: block;
	}
	table.dd th {
		text-align: left;
		border-bottom: 2px solid var(--color-border);
		color: var(--color-text-subtle);
		font-size: var(--text-xs);
		text-transform: uppercase;
		white-space: nowrap;
	}
	table.dd th.num,
	table.dd td.num {
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	table.dd th.center,
	table.dd td.center {
		text-align: center;
	}
	table.dd tbody tr {
		cursor: pointer;
	}
	table.dd tbody tr:hover {
		background: var(--color-surface-accent-wash);
	}
	/* A spacer is not a row: no rule under it, no hover, nothing to click. */
	table.dd tbody tr.vspace,
	table.dd tbody tr.vspace:hover {
		background: none;
		cursor: default;
	}
	table.dd tbody tr.vspace td {
		padding: 0;
		border: 0;
	}
	table.dd tbody tr.picked {
		background: var(--color-surface-selected);
	}
	/* Only the Name cell opens the card; the other columns are facet
	   searches (dumpstar-ozm). Highlight whichever cell the pointer is
	   over so the per-column click target reads clearly. */
	table.dd tbody td.namecol:hover,
	table.dd tbody td.fac:hover {
		background: var(--color-surface-selected);
		color: var(--color-text);
	}
	/* Match the .cardtile.missing treatment so unowned catalog rows read
	   the same way in table view as in grid view. The dimming is carried
	   by the opacity + the grayscale filter, so the text itself can stay
	   on the AA-clearing subtle step. */
	table.dd tbody tr.missing {
		color: var(--color-text-subtle);
		opacity: 0.82;
	}
	table.dd tbody tr.missing img {
		filter: grayscale(0.9) brightness(0.62);
	}
	/* Widths for these two come from the <colgroup>, like every other column —
	   a `width` on the cell would override its `col` under fixed layout and put
	   the two sources of truth back into disagreement. */
	table.dd .cbcol {
		text-align: center;
	}
	table.dd th.qty,
	table.dd td.qty {
		padding-left: 0.35rem;
		padding-right: 0.35rem;
	}
	/* DD-style pill around per-row price values; '—' stays unstyled so empty
	   prices don't draw a box. */
	.pricebox {
		display: inline-block;
		background: var(--color-surface-page);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		padding: 1px 6px;
		font-variant-numeric: tabular-nums;
	}
	.pricedash {
		color: var(--color-text-disabled);
	}
	.sortable {
		cursor: pointer;
		user-select: none;
	}
	.sortable:hover {
		color: var(--color-text);
	}
	.caret {
		color: var(--color-text-accent);
		font-size: 0.65rem;
		margin-left: 0.15rem;
	}
	.qty {
		font-weight: var(--weight-semibold);
		color: var(--color-text);
	}
	.namecell {
		display: flex;
		align-items: center;
		gap: 0.55rem;
		min-width: 0;
	}
	.cardthumb {
		width: 110px;
		height: 36px;
		object-fit: cover;
		object-position: top center;
		border-radius: var(--radius-xs);
		flex-shrink: 0;
		background: var(--color-surface-well);
	}
	.cardname {
		font-weight: var(--weight-medium);
		color: var(--color-text);
	}
	/* Wrap name + inline tags so the tags flow with the text. Without this
	   wrapper they sit as flex siblings of .namecell, and a long-wrapping
	   name shrinks to fill the remaining width, pushing the tags to the
	   right edge of the cell instead of abutting the name.

	   One line, and truncated rather than wrapped, because the virtual
	   scroller's geometry is arithmetic over ONE row height (pd-7z4o): a name
	   that wrapped made its row 3px taller than its neighbours, and 3px times
	   56,635 rows is a scrollbar that lies about where you are. Every other
	   cell already fits inside the thumbnail's height — this was the only one
	   that could outgrow it. The column flexes to all leftover width, so an
	   ellipsis is a narrow-viewport event, and the type cell either side of it
	   ellipsises on the same reasoning. */
	.namebody {
		min-width: 0;
		line-height: 1.25;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	/* Inline tags (variant, non-owned status) — DD card-tag pattern.
	   inline-block + vertical-align lets the pill keep its shape while
	   flowing as inline content inside .namebody, so a wrapped card
	   name has its tags glued to the last line of text.

	   Deliberately not the Badge primitive: these sit inline inside a
	   table cell tuned for density, and Badge's pill padding would reflow
	   the Name column. The tones below use the same state roles Badge's
	   `soft` variant resolves, so the two stay in step. */
	.tag {
		display: inline-block;
		vertical-align: middle;
		margin-left: 0.4rem;
		padding: 1px 4px;
		font-size: 0.62rem;
		font-weight: var(--weight-semibold);
		text-transform: uppercase;
		border-radius: var(--radius-xs);
		border: 1px solid;
		letter-spacing: 0.04em;
		white-space: nowrap;
	}
	.vtag {
		background: var(--color-surface-panel);
		color: var(--color-text-subtle);
		border-color: var(--color-border);
	}
	.stag.t-ordered {
		background: var(--color-warning-surface);
		color: var(--color-warning-text);
		border-color: var(--color-warning-border);
	}
	.stag.t-listed {
		background: var(--color-info-surface);
		color: var(--color-info-text);
		border-color: var(--color-info-border);
	}
	.stag.t-sold,
	.stag.t-traded,
	.stag.t-gifted {
		background: var(--color-success-surface);
		color: var(--color-success-text);
		border-color: var(--color-success-border);
	}
	.stag.t-removed,
	.stag.t-lost {
		background: var(--color-danger-surface);
		color: var(--color-danger-text);
		border-color: var(--color-danger-border);
	}



	/* On a phone the table is just a denser version of itself — no row
	   reflow yet (it would clash with click-to-sort headers). */
	@media (max-width: 540px) {
		.content {
			--gutter-x: var(--space-3);
			--gutter-top: var(--space-3);
			padding: var(--gutter-top) var(--gutter-x) var(--space-8);
		}
		table.dd {
			font-size: var(--text-sm);
			--row-content: 26px;
			/* The declared widths shrink with the type and the padding, or the
			   table would keep its desktop total on a 360px screen and scroll
			   further than it used to. Re-declared rather than derived: they
			   are measurements of a different type size, not a ratio of the
			   desktop ones. The thumbnail is 70px here, so the name floor
			   drops with it. */
			--col-sel: 1.75rem;
			--col-qty: 2rem;
			--col-type: 4.5rem;
			--col-aspects: calc(var(--aspect-n) * 1.25em + 0.7rem + 1px);
			--col-cost: 2.75rem;
			--col-stats: 4.5rem;
			--col-rarity: 4.25rem;
			--col-set: 3.25rem;
			--col-number: 3rem;
			--col-nm: 4rem;
			--col-adj: 4.75rem;
			--col-name-min: 9rem;
		}
		.cardthumb {
			width: 70px;
			height: 26px;
		}
		table.dd th,
		table.dd td {
			padding: 0.3rem 0.35rem;
		}
		/* Names wrap, never truncate (deckdumpster's mobile table). Still ONE
		   row height for the scroller: the row is two lines of name tall, and
		   a one-line name is centred in it. */
		table.dd {
			--row-content: 2.4em;
		}
		.namebody {
			white-space: normal;
			overflow-wrap: anywhere;
			text-overflow: clip;
			line-height: 1.2;
		}
		table.dd td.fac {
			white-space: normal;
			overflow-wrap: anywhere;
			line-height: 1.2;
		}
	}
</style>
