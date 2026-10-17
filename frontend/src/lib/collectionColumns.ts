/**
 * The /collection table's optional columns: which exist, which start hidden,
 * and which are shown given what the rows hold. Name is not here — a row with
 * no name is not a row.
 */

export const COLUMNS = [
	{ key: 'qty', label: 'Qty' },
	{ key: 'type', label: 'Type' },
	{ key: 'traits', label: 'Traits' },
	{ key: 'aspects', label: 'Aspects' },
	{ key: 'cost', label: 'Cost' },
	{ key: 'stats', label: 'Pwr/HP' },
	{ key: 'rarity', label: 'Rarity' },
	{ key: 'set', label: 'Set' },
	{ key: 'number', label: '#' },
	{ key: 'nm', label: 'Price' },
	{ key: 'adj', label: 'Adjusted' }
];

/** Hidden until the reader asks for them. */
export const DEFAULT_HIDDEN = ['qty', 'adj'];

/** At or under this width the table is the phone layout (see the page's CSS). */
export const PHONE_MAX_WIDTH = 540;

/**
 * What a phone starts with: Name and Type, as deckdumpster's mobile table.
 * Everything else is one tap away in the column chooser.
 */
export const PHONE_HIDDEN = COLUMNS.map((c) => c.key).filter((k) => k !== 'type');

/** The stored value is the list of hidden keys; unknown keys are dropped. */
export function parseHidden(raw: string | null, phone = false): string[] {
	const fallback = phone ? PHONE_HIDDEN : DEFAULT_HIDDEN;
	if (raw == null) return [...fallback];
	try {
		const v = JSON.parse(raw);
		if (!Array.isArray(v)) return [...fallback];
		const known = new Set(COLUMNS.map((c) => c.key));
		return v.filter((k) => known.has(k));
	} catch {
		return [...fallback];
	}
}

/**
 * Whether any row's condition-adjusted price differs from its Near Mint one.
 * With none, Adjusted would only repeat NM.
 */
export function anyAdjusted(
	rows: readonly { nm_unit: number | null; market_unit: number | null }[]
): boolean {
	return rows.some((r) => r.market_unit != null && r.nm_unit != null && r.market_unit !== r.nm_unit);
}

/** Columns the table draws: not hidden, and Adjusted only when something is adjusted. */
export function visibleColumns(hidden: readonly string[], adjusted: boolean): string[] {
	return COLUMNS.map((c) => c.key).filter(
		(k) => !hidden.includes(k) && (k !== 'adj' || adjusted)
	);
}
