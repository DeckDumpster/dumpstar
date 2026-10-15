import { test } from 'node:test';
import assert from 'node:assert/strict';
import { printingTag } from '../src/lib/printing-tag.js';

test('a Weekly Play Foil copy in a Weekly Play set reads FOIL', () => {
	assert.equal(printingTag('Weekly Play', 'Foil', 'Jump to Lightspeed: Weekly Play Promos'), 'Foil');
	assert.equal(printingTag('Weekly Play', 'Normal', 'Jump to Lightspeed: Weekly Play Promos'), '');
});

test('Hyperspace Foil in a main set keeps both words', () => {
	assert.equal(printingTag('Hyperspace', 'Foil', 'Spark of Rebellion'), 'Hyperspace Foil');
});

test('Standard shows only its finish', () => {
	assert.equal(printingTag('Standard', 'Foil', 'Spark of Rebellion'), 'Foil');
	assert.equal(printingTag('Standard', 'Normal', 'Spark of Rebellion'), '');
});

test('a program inside a mixed set uses its short code', () => {
	assert.equal(printingTag('Weekly Play', 'Foil', 'Spark of Rebellion'), 'WP Foil');
});
