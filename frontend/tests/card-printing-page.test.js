import { test } from 'node:test';
import assert from 'node:assert/strict';
import { markup } from './support/render.js';
import OtherPrintings from '../src/lib/components/OtherPrintings.svelte';
import {
	otherPrintings,
	printingHref,
	shownPrinting,
	siblingsAt,
	seriesFor
} from '../src/lib/printings.js';

/** @param {string} id @param {string} set @param {string} number @param {string} variant @param {string} finish @param {number} price */
const p = (id, set, number, variant, finish, price) => ({
	printing_id: id,
	set_code: set,
	number,
	variant,
	finish,
	rarity: null,
	image_url: null,
	back_image_url: null,
	owned_count: 0,
	market_price: price,
	tcgplayer_product_id: null,
	is_user_added: false,
	description: null
});

// One card, three places: Standard 5 (Normal+Foil) and Hyperspace 262 in SOR,
// plus a Promo in JTL — priced differently so a leak shows.
const printings = [
	p('sor-5-n', 'sor', '5', 'Standard', 'Normal', 1.11),
	p('sor-5-f', 'sor', '5', 'Standard', 'Foil', 2.22),
	p('sor-262-n', 'sor', '262', 'Hyperspace', 'Normal', 3.33),
	p('jtl-9-n', 'jtl', '9', 'Promo', 'Normal', 4.44)
];

test('other printings link to their own card page, never a search', () => {
	const html = markup(OtherPrintings, { items: otherPrintings(printings, 'sor', '5') });
	const hrefs = [...html.matchAll(/href="([^"]*)"/g)].map((m) => m[1]);
	assert.deepEqual(hrefs.sort(), ['/card/jtl/9', '/card/sor/262']);
	assert.ok(!html.includes('/search'));
});

test('other printings carry no price and omit this set + number', () => {
	const items = otherPrintings(printings, 'sor', '5');
	const html = markup(OtherPrintings, { items });
	assert.ok(!html.includes('$'));
	for (const n of ['3.33', '4.44', '1.11', '2.22']) assert.ok(!html.includes(n));
	assert.ok(!items.some((o) => o.set_code === 'sor' && o.number === '5'));
});

test('finish at the same set + number is picked on the page, not linked', () => {
	assert.deepEqual(
		siblingsAt(printings, 'sor', '5').map((x) => x.printing_id),
		['sor-5-n', 'sor-5-f']
	);
	assert.equal(shownPrinting(printings, 'sor', '5', null)?.printing_id, 'sor-5-n');
	assert.equal(shownPrinting(printings, 'sor', '5', 'sor-5-f')?.printing_id, 'sor-5-f');
	assert.equal(shownPrinting(printings, 'sor', '5', 'jtl-9-n')?.printing_id, 'sor-5-n');
});

test('the chart shows only the shown printing’s own series', () => {
	const series = printings.map((x) => ({
		printing_id: x.printing_id,
		variant: x.variant,
		finish: x.finish,
		price_type: 'market',
		points: [{ date: '2026-10-01', price: x.market_price }]
	}));
	const got = seriesFor(series, 'sor-5-f');
	assert.equal(got.length, 1);
	assert.equal(got[0].points[0].price, 2.22);
	assert.deepEqual(seriesFor(series, null), []);
});

test('printingHref encodes the route segments', () => {
	assert.equal(printingHref({ set_code: 'sor', number: '5/a' }), '/card/sor/5%2Fa');
});
