// Vector rarity marks, traced from the marks printed on SWU cards (the
// bottom-right of the card, beside the set code and number). Each mark is a
// slanted bold letter on a badge with a light outline and a trailing burst:
// a disc and a round burst for Common (a bold bronze C, not the pale slate of the token), a pentagon for Uncommon, a five-point star for Rare, a
// seven-point star for Legendary, a trapezoid for Special. `frame` is the
// outline plus burst, drawn in the text colour; `path` is the letter, drawn in
// the rarity's colour (the publisher's, served by /api/rarities, when the
// catalog has it, else the default here). Starter is not printed as a letter
// and keeps a plain badge.

export type RarityShape = { name: string; color: string; path: string; frame: string };

const BADGE = 'M3 1H15.6L13.6 18.6H0.8Z';

const SHAPES: Record<string, RarityShape> = {
	common: {
		name: 'Common',
		color: 'var(--pd-amber-700)',
		frame: 'M0.9 10.9A9.1 9.1 0 1 1 19.1 10.9 9.1 9.1 0 1 1 0.9 10.9Z M9.4 16.2A7.7 7.7 0 1 1 24.8 16.2 7.7 7.7 0 1 1 9.4 16.2Z',
		path: 'M15.83 6.34A7.4 7.4 0 1 0 15.83 15.46L12.84 13.12A3.6 3.6 0 1 1 12.84 8.68ZM10.4 10H14.4V11.9H10.4Z'
	},
	uncommon: {
		name: 'Uncommon',
		color: 'var(--color-text-subtle)',
		frame: `${BADGE}M14 11 22.6 17 16.5 24 9.5 19Z`,
		path: 'M4.4 3.5H8.4L7.6 11.8Q7.4 13.2 8.6 13.2 9.8 13.2 10 11.8L11.4 3.5H15L13.4 12.6Q12.6 16.8 8.4 16.8 3.2 16.6 3.2 12Z'
	},
	rare: {
		name: 'Rare',
		color: 'var(--color-rarity-special)',
		frame: `${BADGE}M17.3 6.2 18.9 11.9 24.4 14 19.5 17.3 19.2 23.2 14.6 19.5 8.9 21.1 10.9 15.5 7.7 10.6 13.6 10.8Z`,
		path: 'M5.6 3.5H11.2Q14.4 3.5 14.2 7 14 9.6 11.8 10.4L13.6 16.5H10.2L8.8 11H8L7.2 16.5H3.2ZM8.6 6.2 8.2 8.6H10Q11.2 8.6 11.3 7.4 11.2 6.2 10 6.2Z'
	},
	legendary: {
		name: 'Legendary',
		color: 'var(--color-rarity-promo)',
		frame: `${BADGE}M16.3 5 17.9 9.6 22.6 8.9 20.1 12.9 23.6 16.2 18.8 16.7 18.4 21.5 15 18.1 11.1 20.8 11.6 16 7.1 14.6 11.1 12 9.4 7.6 13.9 9.2Z`,
		path: 'M6.2 3.5H10.4L8.8 13H13.4L12.8 16.6H3.4Z'
	},
	special: {
		name: 'Special',
		color: 'var(--color-text-inverse)',
		frame: 'M7.5 1C3.6 1 0.8 3 0.8 6.5 0.8 9 2.6 10.6 4.6 11.2L10 12.6V17H4L3.4 19.2 5 22.4H17.6L18.6 8.4 11.2 8 8 6.9 11.6 6.4 15 7.2 16.2 2.6C13.6 1.2 10.6 1 7.5 1Z',
		path: 'M13 4.4Q10 3.4 7.6 4.2 4.4 5.4 5.4 8.4 6.2 10.2 8.8 10.8 10 11.2 9.8 12 9.4 12.8 8 12.6L5.2 12 4.6 15.4Q8 17 11 16.2 14.6 15 13.6 11.8 12.8 9.6 10.4 9 9 8.6 9.2 7.8 9.6 7 11 7.2L13.4 7.8Z'
	},
	starter: { name: 'Starter', color: 'var(--color-rarity-unranked)', frame: '', path: 'M4 4H20V20H4Z' }
};

const UNKNOWN: RarityShape = { name: '', color: 'var(--color-rarity-unranked)', frame: '', path: 'M12 4A8 8 0 1 0 12 20 8 8 0 1 0 12 4Z' };

export function rarityShape(name: string): RarityShape {
	const s = SHAPES[name.trim().toLowerCase()];
	return s ?? { ...UNKNOWN, name };
}
