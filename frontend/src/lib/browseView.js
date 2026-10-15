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
