import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { markup } from './support/render.js';
import DeckLines from '../src/lib/components/DeckLines.svelte';
import { buildDeckView } from '../src/lib/deckView.ts';

let id = 0;
/** @param {any} o */
const row = (o) => ({
	id: ++id, printing_id: `p-${o.name}`, condition: 'Near Mint', finish: 'Normal', set_code: 'SOR',
	set_name: 'Spark of Rebellion', number: '017', variant: 'Standard', cost: 2, aspects: '["Vigilance"]',
	arenas: '["Ground"]', image_url: `https://img.test/${o.name}.png`, ...o
});
const cards = [
	row({ name: 'Luke', card_type: 'Leader', cost: 5 }),
	row({ name: 'Echo Base', card_type: 'Base', cost: null, aspects: '[]', arenas: null }),
	row({ name: 'Pyke Sentinel', card_type: 'Unit' }),
	row({ name: 'Pyke Sentinel', card_type: 'Unit', printing_id: 'p-Pyke Sentinel' }),
	row({ name: 'X-Wing', card_type: 'Unit', arenas: '["Space"]', cost: 3, aspects: '["Heroism"]' }),
	row({ name: 'Vanquish', card_type: 'Event', cost: 5 })
];

test('groups by type with counts; leader and base pinned', () => {
	const v = buildDeckView(cards);
	assert.deepEqual(v.groups.map((g) => [g.label, g.count]), [['Ground Units', 2], ['Space Units', 1], ['Events', 1]]);
	assert.equal(v.groups[0].lines[0].qty, 2);
	assert.equal(v.leaders.length, 1);
	assert.equal(v.bases.length, 1);
	assert.equal(v.curve[2], 2);
});

test('page shows images, cost, aspects, and no set/number/condition text', () => {
	const html = markup(DeckLines, { view: buildDeckView(cards) });
	assert.ok(html.includes('src="https://img.test/Luke.png"'));
	assert.ok(!html.includes('src="https://img.test/Echo Base.png"'), 'no base art');
	assert.match(html, /Ground Units/);
	assert.match(html, /aria-label="Cost 2"/);
	assert.match(html, /alt="Vigilance"/);
	for (const t of ['Spark of Rebellion', 'Near Mint', '017', 'SOR']) {
		const text = html.replace(/<[^>]*>/g, ' ');
		assert.ok(!text.includes(t), `text shows ${t}`);
	}
});

test('each line links to its printing at its condition', () => {
	const html = markup(DeckLines, { view: buildDeckView(cards) });
	assert.match(html, /href="\/card\/SOR\/017\?finish=Normal&amp;condition=Near\+Mint"/);
	assert.match(html, /data-printing="p-X-Wing"/);
	assert.match(html, /data-condition="Near Mint"/);
});

test('leader and base are lines of their own section before the type groups', () => {
	const html = markup(DeckLines, { view: buildDeckView(cards) });
	const sec = html.indexOf('aria-label="Leader and base"');
	assert.ok(sec > 0 && sec < html.indexOf('Ground Units'));
	assert.ok(html.lastIndexOf('data-printing="p-Luke"') > sec);
	assert.ok(html.indexOf('data-printing="p-Echo Base"') > sec);
});

test('the preview follows the leader art in the pane', () => {
	const html = markup(DeckLines, { view: buildDeckView(cards) });
	const img = html.indexOf('src="https://img.test/Luke.png"');
	const prev = html.indexOf('class="preview');
	assert.ok(img > 0 && prev > img);
	assert.ok(prev < html.indexOf('aria-label="Deck summary"'));
});

test('groups flow into columns when there is room', () => {
	const css = readFileSync(new URL('../src/lib/components/DeckLines.svelte', import.meta.url), 'utf8');
	assert.match(css, /\.groups\s*\{[^}]*column-width:/);
});
