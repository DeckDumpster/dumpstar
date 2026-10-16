import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const src = readFileSync(new URL('../src/routes/browse/+page.svelte', import.meta.url), 'utf8');

test('browse ports the Jump-to rail, collapsible groups and chip row', () => {
	assert.match(src, /<aside class="navcol">/);
	assert.match(src, /Jump to/);
	assert.match(src, /aria-expanded=\{!collapsed\[g\.category\]\}/);
	assert.match(src, /position: sticky/);
	assert.match(src, /@media \(max-width: 720px\)[\s\S]*flex-wrap: wrap/);
	const narrow = src.slice(src.indexOf('@media (max-width: 720px)'));
	assert.doesNotMatch(narrow, /overflow-x: auto/);
});
