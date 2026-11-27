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

/** `1,635 distinct printings`: the words the homepage and the collection page share. */
export function distinctPrintings(n: number): string {
	return `${count(n)} distinct ${n === 1 ? 'printing' : 'printings'}`;
}

/** The collection page's count line: copies owned and distinct printings, then
 *  the value of those copies when there is one. */
export function collectionCountLine(
	copies: number,
	printings: number,
	value: number | null
): string {
	const line = `${count(copies)} ${copies === 1 ? 'copy' : 'copies'} owned · ${distinctPrintings(printings)}`;
	return value === null ? line : `${line}, ${money(value)}`;
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

/** The Type cell's two lines: the card type, then its arena(s) beneath
 *  (`Unit` / `Ground`). A card with no arena has the type alone. */
export function typeLines(
	cardType: string | null | undefined,
	arenas: string | null | undefined
): { type: string; arena: string } {
	return { type: cardType ?? '', arena: jsonList(arenas).join(' \u00b7 ') };
}

/** The Traits column: every trait on one line, `Underworld · Droid · Bounty Hunter`. */
export function traitsLine(traits: string | null | undefined): string {
	return jsonList(traits).join(' \u00b7 ');
}
