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
