/** Set-page view mode and the leaders-and-bases split. */

/** @typedef {'binder' | 'card'} ViewMode */

/**
 * Card view is the default; only an explicit `view=binder` selects binder.
 * @param {string | null | undefined} raw
 * @returns {ViewMode}
 */
export function parseView(raw) {
	return raw === 'binder' ? 'binder' : 'card';
}

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
 */
export function pageAfterResize(page, oldLayout, newLayout) {
	const first = (Math.max(1, page) - 1) * oldLayout;
	return Math.floor(first / newLayout) + 1;
}
