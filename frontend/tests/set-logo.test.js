import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { markup } from './support/render.js';
import SetLogo from '../src/lib/components/SetLogo.svelte';

// Server render has no fetch resolved yet, so the component shows nothing
// rather than a broken image; the wiring is asserted on the sources.
test('SetLogo renders no <img> until the set has a retrieved logo', () => {
	assert.doesNotMatch(markup(SetLogo, { code: 'SOR', name: 'Spark of Rebellion' }), /<img/);
});

test('the collection Set cell, the card page and the /browse tile each show the set logo', () => {
	const read = (/** @type {string} */ f) => readFileSync(new URL(f, import.meta.url), 'utf8');
	const collection = read('../src/routes/collection/+page.svelte');
	assert.equal((collection.match(/<SetLogo code=\{a\.set_code\}/g) ?? []).length, 2);
	assert.match(read('../src/lib/components/CardDetailView.svelte'), /<SetLogo code=\{card\.set_code\}/);
	assert.match(read('../src/routes/browse/+page.svelte'), /<img class="logo" src=\{set\.logo_url\}/);
	assert.match(read('../src/lib/components/SetLogo.svelte'), /<img class="setmark" \{src\}/);
});
