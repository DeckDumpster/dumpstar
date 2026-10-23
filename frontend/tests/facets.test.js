import { test } from 'node:test';
import assert from 'node:assert/strict';
import { facetHref, setFacetContext } from '../src/lib/facets.ts';

test('a facet href carries only q, never an all param', () => {
	setFacetContext('');
	assert.equal(facetHref('trait', 'Rebel'), '/collection?q=trait%3ARebel');
	assert.doesNotMatch(facetHref('artist', 'Jane Doe'), /all=/);
});

test('a facet click from a plain query is an owned-only search', () => {
	setFacetContext('set:sor');
	assert.equal(facetHref('aspect', 'Vigilance'), '/collection?q=aspect%3AVigilance');
});

test('a facet click from an is:unowned query keeps the predicate', () => {
	setFacetContext('is:unowned set:sor');
	assert.equal(facetHref('trait', 'Rebel'), '/collection?q=is%3Aunowned+trait%3ARebel');
	setFacetContext('');
});
