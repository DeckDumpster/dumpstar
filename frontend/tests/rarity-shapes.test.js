import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rarityShape } from '../src/lib/rarityShapes.ts';

test('draws every SWU rarity as its own vector path', () => {
	const names = ['Common', 'Uncommon', 'Rare', 'Legendary', 'Special', 'Starter'];
	const paths = names.map((n) => rarityShape(n).path);
	for (const p of paths) assert.ok(p.length > 0);
	assert.equal(new Set(paths).size, names.length);
});

test('is case-insensitive and still draws a rarity it does not know', () => {
	assert.equal(rarityShape('LEGENDARY').name, 'Legendary');
	assert.ok(rarityShape('Mythic').path.length > 0);
});
