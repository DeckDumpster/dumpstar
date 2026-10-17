import badges from './printing-badges.json' with { type: 'json' };

/**
 * The single-letter badges for a printing: its variant's letters, then F when
 * the finish is foil. Each carries the full name for a tooltip and screen readers.
 * @param {string} variant
 * @param {string} finish
 * @returns {{ letter: string, label: string }[]}
 */
export function printingBadges(variant, finish) {
	const letters = [...(/** @type {Record<string, string[]>} */ (badges.variants)[variant] ?? [])];
	const foil = /** @type {Record<string, string>} */ (badges.finish)[finish];
	if (foil && !letters.includes(foil)) letters.push(foil);
	const names = /** @type {Record<string, string>} */ (badges.letters);
	return letters.map((letter) => ({ letter, label: names[letter] ?? letter }));
}
