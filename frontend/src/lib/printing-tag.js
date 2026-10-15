import tags from './printing-tags.json' with { type: 'json' };

/** @param {string} s */
const words = (s) => s.toLowerCase().match(/[a-z0-9]+/g) ?? [];

/**
 * The collection tag for a printing: only what its set does not already say.
 * The variant comes from `printing-tags.json` (dropped when the set name carries
 * all its words), then the finish when it is not the plain one. Empty when
 * there is nothing to add.
 * @param {string} variant
 * @param {string} finish
 * @param {string} setName
 */
export function printingTag(variant, finish, setName) {
	const known = /** @type {Record<string, string>} */ (tags.variants)[variant];
	const inSet = new Set(words(setName));
	const vw = words(variant);
	const redundant = vw.length > 0 && vw.every((w) => inSet.has(w));
	const parts = [redundant ? '' : (known ?? variant), finish === 'Normal' || !finish ? '' : finish];
	return parts.filter(Boolean).join(' ');
}
