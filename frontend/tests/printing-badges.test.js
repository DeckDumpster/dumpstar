import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { printingBadges } from '../src/lib/printing-badges.js';
import badges from '../src/lib/printing-badges.json' with { type: 'json' };

const dir = new URL('../../fixtures/swu/', import.meta.url);

/** Every variant name the captured official catalog carries, finish suffix removed. */
function catalogVariants() {
	const out = new Set();
	for (const f of readdirSync(dir).filter((n) => n.startsWith('official-trim-'))) {
		const text = gunzipSync(readFileSync(new URL(f, dir))).toString();
		for (const m of text.matchAll(/"variantTypes":\{"data":\[\{"attributes":\{"name":"([^"]+)"/g)) {
			out.add(m[1].replace(/ Foil$/, ''));
		}
	}
	return out;
}

test('every variant the catalog carries has a badge row', () => {
	const seen = catalogVariants();
	assert.ok(seen.size >= 5, `read only ${[...seen]}`);
	for (const v of seen) assert.ok(v in badges.variants, `variant "${v}" has no badge letter`);
});

test('every letter is named and unique per meaning', () => {
	for (const letters of Object.values(badges.variants))
		for (const l of letters) assert.ok(badges.letters[l], `letter ${l} is unnamed`);
});

test('badges carry full names', () => {
	assert.deepEqual(printingBadges('Standard', 'Normal'), []);
	assert.deepEqual(printingBadges('Hyperspace', 'Foil'), [
		{ letter: 'H', label: 'Hyperspace' },
		{ letter: 'F', label: 'Foil' }
	]);
	assert.deepEqual(printingBadges('Foil Prestige', 'Foil').map((b) => b.letter), ['P', 'F']);
});
