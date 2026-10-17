import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { markup } from './support/render.js';
import TypeLines from '../src/lib/components/TypeLines.svelte';
import TraitLinks from '../src/lib/components/TraitLinks.svelte';

test('the type cell renders the type link and the arena as two elements, the second secondary', () => {
	const html = markup(TypeLines, { cardType: 'Unit', arenas: '["Ground"]' });
	assert.match(html, /<a class="[^"]*t-type[^"]*"[^>]*href="\/collection\?q=type%3AUnit&amp;all=1"[^>]*>Unit<\/a>/);
	assert.match(html, /<span class="[^"]*t-sub[^"]*"><a class="t-arena" href="\/collection\?q=arena%3AGround&amp;all=1"[^>]*>Ground<\/a><\/span>/);
	assert.ok(html.indexOf('Unit') < html.indexOf('Ground'));
});

test('a card with no arena shows only its type', () => {
	assert.doesNotMatch(markup(TypeLines, { cardType: 'Base', arenas: null }), /t-sub/);
});

test('each trait is its own anchor to a trait search', () => {
	const html = markup(TraitLinks, { traits: '["Underworld","Droid","Bounty Hunter"]' });
	const anchors = [...html.matchAll(/<a [^>]*href="([^"]+)"[^>]*>([^<]+)<\/a>/g)];
	assert.deepEqual(
		anchors.map((m) => m[2]),
		['Underworld', 'Droid', 'Bounty Hunter']
	);
	assert.match(anchors[2][1], /q=trait%3A%22Bounty\+Hunter%22/);
	assert.match(anchors[0][1], /q=trait%3AUnderworld/);
});

test('the Name cell puts a subtitle below the name in the secondary class, and omits it when absent', () => {
	const src = readFileSync(new URL('../src/routes/collection/+page.svelte', import.meta.url), 'utf8');
	const cells = src.match(/\{#if a\.subtitle\}<span class="cardsub">\{a\.subtitle\}<\/span>\{\/if\}/g) ?? [];
	assert.equal(cells.length, 2);
	assert.match(src, /\.cardsub \{[^}]*display: block;[^}]*var\(--color-text-subtle\)/);
});
