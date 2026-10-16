// sp-yc7gp1: a binder slot is one collector number, so a card's Standard and
// Hyperspace printings are two slots with one card_id. A keyed {#each} over
// slots keyed by card_id duplicated keys as soon as a sort (Price) put both on
// one page, and the set page stopped responding.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const src = readFileSync(new URL('../src/routes/browse/[set]/+page.svelte', import.meta.url), 'utf8');

test('slot lists are keyed by collector number, never card_id', () => {
	const keyed = src.match(/\{#each [^}]*\(([^)]*)\)\}/g) ?? [];
	const slotLists = keyed.filter((k) => /\bslot\b/.test(k) && !/printings/.test(k));
	assert.ok(slotLists.length === 3, `expected the slot lists, found ${slotLists.length}`);
	for (const k of slotLists) assert.match(k, /\(slot\.number\)\}$/, k);
});
