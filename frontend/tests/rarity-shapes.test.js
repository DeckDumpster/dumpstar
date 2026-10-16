import { describe, it, expect } from 'vitest';
import { rarityShape } from '../src/lib/rarityShapes';

describe('rarityShape', () => {
	it('draws every SWU rarity as its own vector path', () => {
		const names = ['Common', 'Uncommon', 'Rare', 'Legendary', 'Special', 'Starter'];
		const paths = names.map((n) => rarityShape(n).path);
		for (const p of paths) expect(p.length).toBeGreaterThan(0);
		expect(new Set(paths).size).toBe(names.length);
	});

	it('is case-insensitive and still draws a rarity it does not know', () => {
		expect(rarityShape('LEGENDARY').name).toBe('Legendary');
		expect(rarityShape('Mythic').path.length).toBeGreaterThan(0);
	});
});
