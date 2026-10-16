import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const src = readFileSync(new URL('../src/routes/collection/+page.svelte', import.meta.url), 'utf8');

test('each aspect icon in a collection row links to its own aspect, not the first', () => {
	const cells = src.match(/facetCell\(e, 'aspect', n\)/g) ?? [];
	assert.equal(cells.length, 2);
});
