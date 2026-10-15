// Shared number formatting. Everything user-facing routes through here so
// thousands separators are consistent and locale is pinned (the box may
// run under any system locale; we always want en-US grouping + a dot
// decimal). Built once at module load — Intl.NumberFormat instances are
// reusable and cheap to call.

const USD = new Intl.NumberFormat('en-US', {
	minimumFractionDigits: 2,
	maximumFractionDigits: 2
});

const INT = new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 });

/** A USD price: `$1,234.50`. Null/undefined renders as an em dash so
 *  callers don't each re-implement the "no value" case. */
export function money(n: number | null | undefined): string {
	return n == null ? '—' : `$${USD.format(n)}`;
}

/** A thousands-separated integer count: `1,234`. Null/undefined → em dash. */
export function count(n: number | null | undefined): string {
	return n == null ? '—' : INT.format(n);
}

/** A printing's human label: its variant, plus the finish when it is not the
 *  plain one — `Standard`, `Standard Foil`, `Hyperspace Foil`. */
export function printingLabel(variant: string, finish: string): string {
	return finish === 'Normal' || !finish ? variant : `${variant} ${finish}`;
}

export { printingTag } from './printing-tag.js';

/** Parse a JSON-array column (`aspects`, `traits`, `arenas`) the API passes
 *  through as a string. Anything that is not an array of strings is empty. */
export function jsonList(raw: string | null | undefined): string[] {
	if (!raw) return [];
	try {
		const v: unknown = JSON.parse(raw);
		return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
	} catch {
		return [];
	}
}
