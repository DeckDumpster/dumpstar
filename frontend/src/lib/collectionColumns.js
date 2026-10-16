/**
 * The /collection table's optional columns: which exist, which start hidden,
 * and which are shown given what the rows hold. Name is not here — a row with
 * no name is not a row.
 */

export const COLUMNS = [
	{ key: 'qty', label: 'Qty' },
	{ key: 'type', label: 'Type' },
	{ key: 'aspects', label: 'Aspects' },
	{ key: 'cost', label: 'Cost' },
	{ key: 'stats', label: 'Pwr/HP' },
	{ key: 'rarity', label: 'Rarity' },
	{ key: 'set', label: 'Set' },
	{ key: 'number', label: '#' },
	{ key: 'nm', label: 'NM' },
	{ key: 'adj', label: 'Adjusted' }
];

/** Hidden until the reader asks for them. */
export const DEFAULT_HIDDEN = ['adj'];

/** The stored value is the list of hidden keys; unknown keys are dropped. */
export function parseHidden(raw) {
	if (raw == null) return [...DEFAULT_HIDDEN];
	try {
		const v = JSON.parse(raw);
		if (!Array.isArray(v)) return [...DEFAULT_HIDDEN];
		const known = new Set(COLUMNS.map((c) => c.key));
		return v.filter((k) => known.has(k));
	} catch {
		return [...DEFAULT_HIDDEN];
	}
}

/**
 * Whether any row's condition-adjusted price differs from its Near Mint one.
 * With none, Adjusted would only repeat NM.
 */
export function anyAdjusted(rows) {
	return rows.some((r) => r.market_unit != null && r.nm_unit != null && r.market_unit !== r.nm_unit);
}

/** Columns the table draws: not hidden, and Adjusted only when something is adjusted. */
export function visibleColumns(hidden, adjusted) {
	return COLUMNS.map((c) => c.key).filter(
		(k) => !hidden.includes(k) && (k !== 'adj' || adjusted)
	);
}
