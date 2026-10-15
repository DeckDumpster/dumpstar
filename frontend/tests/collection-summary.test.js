import { test } from 'node:test';
import assert from 'node:assert/strict';
import CollectionSummary from '../src/lib/components/CollectionSummary.svelte';
import { markup } from './support/render.js';

const base = {
	loading: false,
	valued: false,
	ownedCards: 0,
	valuedCards: '0',
	ownedLabel: '0',
	ownedMeta: '',
	valueMeta: '',
	marketLabel: '$0.00',
	costLabel: '$0.00',
	unrealized: null,
	unrealizedLabel: '$0.00'
};

test('owned cards with no valuation yet show the count and the note, not the empty state', () => {
	const html = markup(CollectionSummary, { ...base, ownedCards: 2, ownedLabel: '2' });
	assert.doesNotMatch(html, /No cards registered yet/);
	assert.doesNotMatch(html, /Import a CSV/);
	assert.match(html, /Cards owned/);
	assert.match(html, /<strong[^>]*>2<\/strong>/);
	assert.match(html, /after the next valuation/);
	assert.match(html, /07:30 UTC/);
});

test('owning nothing shows the empty state', () => {
	const html = markup(CollectionSummary, base);
	assert.match(html, /No cards registered yet/);
	assert.doesNotMatch(html, /next valuation/);
});

test('nothing is shown while loading', () => {
	const html = markup(CollectionSummary, { ...base, loading: true });
	assert.doesNotMatch(html, /No cards registered yet/);
});

test('a valued collection shows the figures', () => {
	const html = markup(CollectionSummary, {
		...base,
		valued: true,
		ownedCards: 2,
		valuedCards: '2',
		marketLabel: '$10.00',
		unrealized: 6,
		unrealizedLabel: '$6.00'
	});
	assert.match(html, /Market value/);
	assert.doesNotMatch(html, /No cards registered yet/);
});
