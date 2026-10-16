// Vector rarity marks. A rarity's look is a shape and a colour: the colour is
// the publisher's (served by /api/rarities) when the catalog has it, else the
// default here, so a rarity the catalog lacks still draws rather than falling
// back to its name.

export type RarityShape = { name: string; color: string; path: string };

const SHAPES: Record<string, RarityShape> = {
	common: { name: 'Common', color: 'var(--color-rarity-common)', path: 'M12 3 21 12 12 21 3 12Z' },
	uncommon: { name: 'Uncommon', color: 'var(--color-rarity-uncommon)', path: 'M12 2 22 8 22 16 12 22 2 16 2 8Z' },
	rare: { name: 'Rare', color: 'var(--color-rarity-rare)', path: 'M12 2 14.9 8.6 22 9.3 16.6 14 18.2 21 12 17.3 5.8 21 7.4 14 2 9.3 9.1 8.6Z' },
	legendary: { name: 'Legendary', color: 'var(--color-rarity-legendary)', path: 'M12 1 15 9 23 12 15 15 12 23 9 15 1 12 9 9Z' },
	special: { name: 'Special', color: 'var(--color-rarity-special)', path: 'M12 2 22 12 12 22 2 12 12 2ZM12 7 7 12 12 17 17 12Z' },
	starter: { name: 'Starter', color: 'var(--color-rarity-neutral)', path: 'M4 4H20V20H4Z' }
};

const UNKNOWN: RarityShape = { name: '', color: 'var(--color-rarity-neutral)', path: 'M12 4A8 8 0 1 0 12 20 8 8 0 1 0 12 4Z' };

export function rarityShape(name: string): RarityShape {
	const s = SHAPES[name.trim().toLowerCase()];
	return s ?? { ...UNKNOWN, name };
}
