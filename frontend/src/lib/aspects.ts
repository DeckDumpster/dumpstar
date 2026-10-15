// The six Star Wars: Unlimited aspects as the official icons. The list, the
// icon files, names, colours and sort order are the remedial asset set in
// data/remedial/aspects/manifest.json — this module only loads it. Nothing
// here names an aspect.
import manifest from '../../../data/remedial/aspects/manifest.json' with { type: 'json' };

export interface Aspect {
	name: string;
	file: string;
	colour: string;
	sort_value: number;
}

/** Aspects in the official order. */
export const ASPECTS: Aspect[] = [...manifest.aspects].sort((a, b) => a.sort_value - b.sort_value);

const BY_NAME = new Map(ASPECTS.map((a) => [a.name.toLowerCase(), a]));

/** The manifest row for an aspect name, or undefined for a name it lacks. */
export function aspectOf(name: string): Aspect | undefined {
	return BY_NAME.get(name.toLowerCase());
}

/** Where the static icon is served (frontend/scripts/sync-aspects.js copies
 *  the set to `static/aspects/` at build time). */
export function aspectIconUrl(a: Aspect): string {
	return `/aspects/${a.file}`;
}

/** Names in the official order; names the manifest lacks keep their order,
 *  after the known ones. */
export function sortAspects(names: string[]): string[] {
	const rank = (n: string) => aspectOf(n)?.sort_value ?? Number.MAX_SAFE_INTEGER;
	return [...names].sort((a, b) => rank(a) - rank(b));
}
