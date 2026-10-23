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

const src = readFileSync(new URL('../src/lib/components/DeckLines.svelte', import.meta.url), 'utf8');
const pack = [...cards.slice(0, 2), row({ name: 'Jump Base', card_type: 'Base', cost: null, aspects: '[]', arenas: null }), ...cards.slice(2)];
const slotCount = (/** @type {string} */ h) => (h.match(/data-slot="preview"/g) ?? []).length;

test('leader and base are separate sections before the type groups', () => {
	const html = markup(DeckLines, { view: buildDeckView(pack) });
	const lead = html.indexOf('aria-label="Leader"');
	const base = html.indexOf('aria-label="Base"');
	assert.ok(lead > 0 && base > lead && base < html.indexOf('Ground Units'));
	assert.ok(!html.includes('Leader and base'));
	assert.ok(html.lastIndexOf('data-printing="p-Luke"') > lead && html.lastIndexOf('data-printing="p-Luke"') < base);
	assert.ok(html.indexOf('data-printing="p-Echo Base"') > base);
	assert.ok(html.indexOf('data-printing="p-Jump Base"') > base);
});

test('a line name opens the modal without navigating', () => {
	assert.match(src, /class="name"[\s\S]*?preventDefault\(\);\s*open = l;/);
	assert.match(src, /<Modal setCode=\{open\.setCode\} number=\{open\.number\}/);
});

test('the pane holds exactly one card image, the leader when idle', () => {
	const html = markup(DeckLines, { view: buildDeckView(pack) });
	const pane = html.slice(html.indexOf('aria-label="Card preview"'), html.indexOf('aria-label="Deck summary"'));
	assert.equal((pane.match(/<img/g) ?? []).length, 1);
	assert.equal(slotCount(html), 1);
	assert.ok(pane.includes('src="https://img.test/Luke.png"'));
});

test('a leader previews its back side', () => {
	const v = buildDeckView([row({ name: 'Luke', card_type: 'Leader', back_image_url: 'https://img.test/Luke-back.png' }), ...cards.slice(2)]);
	const html = markup(DeckLines, { view: v });
	assert.ok(html.includes('src="https://img.test/Luke-back.png"'));
	assert.ok(!html.includes('src="https://img.test/Luke.png"'));
	assert.match(html, /data-side="back"/);
});

test('a base preview carries the landscape rotation', () => {
	assert.match(src, /data-rotate=\{isBase \? 'landscape' : undefined\}/);
	assert.match(src, /\[data-rotate='landscape'\] img\s*\{[^}]*rotate\(-90deg\)/);
	const html = markup(DeckLines, { view: buildDeckView([row({ name: 'Echo Base', card_type: 'Base', cost: null }), ...cards.slice(2)]) });
	assert.equal((html.match(/data-rotate/g) ?? []).length, 0, 'idle slot with no leader shows nothing rotated');
});

test('clicking the slot opens the modal for the shown card', () => {
	assert.match(src, /class="preview"[\s\S]*?onclick=\{\(\) => shown && \(open = shown\)\}/);
});

test('groups flow into columns when there is room', () => {
	const css = readFileSync(new URL('../src/lib/components/DeckLines.svelte', import.meta.url), 'utf8');
	assert.match(css, /\.groups\s*\{[^}]*column-width:/);
});
