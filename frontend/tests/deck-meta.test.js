import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { markup } from './support/render.js';
import DeckMeta from '../src/lib/components/DeckMeta.svelte';
import { deckCrumbs } from '../src/lib/deckCrumbs.ts';

const deck = { owner: 'Ryan', format: 'Premier', state: 'idea', group: 'SWU Jumpstart Cube' };
const groups = [{ id: 7, name: 'SWU Jumpstart Cube' }];
const props = { deck, groups, states: ['idea', 'ready', 'built'], busy: false, onState() {}, onGroup() {} };

test('breadcrumbs end in the name, not an id', () => {
	const c = deckCrumbs('Rebels');
	assert.deepEqual(c.map((x) => x.label), ['Decks', 'Rebels']);
	assert.equal(c[1].href, undefined);
});

test('pages register their name crumb', () => {
	for (const f of ['decks/[id]', 'deck-groups/[id]']) {
		const src = readFileSync(new URL(`../src/routes/${f}/+page.svelte`, import.meta.url), 'utf8');
		assert.match(src, /breadcrumbs\.set\(deckCrumbs\(detail\.(deck|group)\.name\)\)/);
	}
});

test('header metadata has no unstyled anchor and no bare labels', () => {
	const html = markup(DeckMeta, props);
	const anchors = html.match(/<a\b[^>]*>/g) ?? [];
	assert.equal(anchors.length, 1);
	assert.match(anchors[0], /class="[^"]*group-link/);
	assert.match(anchors[0], /href="\/deck-groups\/7"/);
	assert.match(html, /Lifecycle/);
	assert.match(html, /class="[^"]*field/);
});
