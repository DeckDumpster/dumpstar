import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rarityShape } from '../src/lib/rarityShapes.ts';

const OLD_GEOMETRY = [
	'M12 3 21 12 12 21 3 12Z',
	'M12 2 22 8 22 16 12 22 2 16 2 8Z',
	'M12 2 14.9 8.6 22 9.3 16.6 14 18.2 21 12 17.3 5.8 21 7.4 14 2 9.3 9.1 8.6Z',
	'M12 1 15 9 23 12 15 15 12 23 9 15 1 12 9 9Z',
	'M12 2 22 12 12 22 2 12 12 2ZM12 7 7 12 12 17 17 12Z'
];
const PRINTED = ['Common', 'Uncommon', 'Rare', 'Legendary', 'Special'];

test('draws every SWU rarity as its own vector path', () => {
	const names = [...PRINTED, 'Starter'];
	const paths = names.map((n) => rarityShape(n).path);
	for (const p of paths) assert.ok(p.length > 0);
	assert.equal(new Set(paths).size, names.length);
});

test('printed rarities are traced glyphs on a framed badge, not the old geometric shapes', () => {
	for (const n of PRINTED) {
		const s = rarityShape(n);
		assert.ok(!OLD_GEOMETRY.includes(s.path), `${n} still uses a generic shape`);
		// A traced letter has curves or many vertices; a polygon of <8 points is a shape.
		const points = (s.path.match(/-?\d+(\.\d+)?/g) ?? []).length / 2;
		assert.ok(points >= 8, `${n} glyph has too few points (${points})`);
		assert.ok(s.frame.length > 0, `${n} lacks the printed outline`);
	}
});

test('the Rare glyph is an R: a counter hole, and a five-point star burst', () => {
	const s = rarityShape('Rare');
	assert.equal(s.path.split('M').length - 1, 2, 'R has outline and a counter');
	assert.equal(s.frame.split('M').length - 1, 2);
	assert.ok(s.frame.includes('24.4 14'));
});

test('is case-insensitive and still draws a rarity it does not know', () => {
	assert.equal(rarityShape('LEGENDARY').name, 'Legendary');
	assert.ok(rarityShape('Mythic').path.length > 0);
});
