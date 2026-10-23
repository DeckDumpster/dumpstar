/** Set-page leaders-and-bases grouping and paging helpers. */

/**
 * Split a page's slots into the landscape section (leaders, then bases — the
 * server puts them first) and the portrait cards below. Order is preserved.
 * @template {{ landscape: boolean }} T
 * @param {T[]} slots
 * @returns {{ landscape: T[], portrait: T[] }}
 */
export function splitLandscape(slots) {
	return {
		landscape: slots.filter((s) => s.landscape),
		portrait: slots.filter((s) => !s.landscape)
	};
}

/**
 * Cut a page's slots, in the order given, into runs: each maximal run of
 * consecutive Leaders is a `leaders` band, each of Bases a `bases` band, and
 * everything between is a `portrait` run. Nothing is reordered or moved
 * across a boundary; a run is only ever a contiguous slice.
 * @template {{ landscape: boolean, card_type?: string | null }} T
 * @param {T[]} slots
 * @returns {{ kind: 'leaders' | 'bases' | 'portrait', slots: T[], start: number }[]}
 */
export function bandRuns(slots) {
	/** @type {{ kind: 'leaders' | 'bases' | 'portrait', slots: T[], start: number }[]} */
	const runs = [];
	slots.forEach((s, i) => {
		const kind = !s.landscape ? 'portrait' : s.card_type === 'Base' ? 'bases' : 'leaders';
		const last = runs[runs.length - 1];
		if (last && last.kind === kind) last.slots.push(s);
		else runs.push({ kind, slots: [s], start: i });
	});
	return runs;
}

/** Rows per binder page; the page size is `cols * BROWSE_ROWS`. */
export const BROWSE_ROWS = 3;
/** Most cards per row. `MAX_COLS * BROWSE_ROWS` equals the backend's BINDER_MAX_LAYOUT. */
export const MAX_COLS = 8;
export const DEFAULT_COLS = 3;

/**
 * Clamp a cards-per-row value into 1..MAX_COLS; junk falls back to the default.
 * @param {number} n
 * @returns {number}
 */
export function clampCols(n) {
	if (!Number.isFinite(n)) return DEFAULT_COLS;
	return Math.min(MAX_COLS, Math.max(1, Math.trunc(n)));
}

/**
 * The page that holds the first card of `page` at `oldLayout` cards per page,
 * once the page size becomes `newLayout`. Changing cards per row keeps the
 * reader at the same place in the set instead of resetting to page 1.
 * @param {number} page
 * @param {number} oldLayout
 * @param {number} newLayout
 * @returns {number}
 */
export function pageAfterResize(page, oldLayout, newLayout) {
	const first = (Math.max(1, page) - 1) * oldLayout;
	return Math.floor(first / newLayout) + 1;
}

/** The price shown on a tile: the dearest owned printing's market price, else
 *  the dearest printing's; null when nothing is priced.
 *  @param {{ printings: { owned_count: number, market_price: number | null }[] }} slot */
export function slotPrice(slot) {
	const best = (/** @type {typeof slot.printings} */ ps) => {
		const v = ps.map((p) => p.market_price).filter((n) => n != null);
		return v.length ? Math.max(.../** @type {number[]} */ (v)) : null;
	};
	return best(slot.printings.filter((p) => p.owned_count > 0)) ?? best(slot.printings);
}

/** Whether the user owns a foil printing of the slot.
 *  @param {{ printings: { owned_count: number, finish: string }[] }} slot */
export function ownedFoil(slot) {
	return slot.printings.some((p) => p.finish === 'Foil' && p.owned_count > 0);
}

/** Finishes in increasing order of rarity; any finish not listed is rarer than all of them. */
const FINISH_ORDER = ['Normal', 'Foil'];

/** Sort rank of a finish: Normal first, then by rarity. @param {string} finish */
export function finishRank(finish) {
	const i = FINISH_ORDER.indexOf(finish);
	return i < 0 ? FINISH_ORDER.length : i;
}

/** Pip colour for a finish: Normal grey, Foil light blue, anything rarer purple.
 *  @param {string} finish */
export function finishColor(finish) {
	if (finish === 'Normal') return 'var(--color-chart-unknown)';
	if (finish === 'Foil') return 'var(--color-info-text)';
	return 'var(--color-rarity-holo)';
}

/** A slot's printings as tile pips: Normal at the left, rarer finishes to its
 *  right; ties (same finish, different variant) keep the given order.
 *  @template {{ finish: string }} T
 *  @param {T[]} printings
 *  @returns {(T & { color: string })[]} */
export function orderedPips(printings) {
	return printings
		.map((p, i) => ({ p, i }))
		.sort((a, b) => finishRank(a.p.finish) - finishRank(b.p.finish) || a.i - b.i)
		.map(({ p }) => ({ ...p, color: finishColor(p.finish) }));
}

/** The price on a tile. A card with any copy owned always shows its dearest
 *  owned printing's price, whatever pip was clicked. An unowned card shows
 *  nothing until a pip is selected, then that printing's price.
 *  @param {{ printings: { printing_id: string | number, owned_count: number, market_price: number | null }[] }} slot
 *  @param {string | number | null} selectedId */
export function tilePrice(slot, selectedId) {
	if (!slot.printings.some((p) => p.owned_count > 0)) {
		if (selectedId == null) return null;
		return slot.printings.find((p) => p.printing_id === selectedId)?.market_price ?? null;
	}
	const v = slot.printings
		.filter((p) => p.owned_count > 0)
		.map((p) => p.market_price)
		.filter((n) => n != null);
	return v.length ? Math.max(.../** @type {number[]} */ (v)) : null;
}
