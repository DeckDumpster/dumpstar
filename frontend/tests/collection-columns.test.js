import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { anyAdjusted, parseHidden, visibleColumns } from '../src/lib/collectionColumns.ts';

test('Qty and Adjusted start hidden', () => {
	assert.deepEqual(parseHidden(null), ['qty', 'adj']);
	assert.deepEqual(parseHidden('garbage'), ['qty', 'adj']);
	assert.ok(!visibleColumns(parseHidden(null), true).includes('qty'));
});

test('a stored choice that shows Qty wins over the default', () => {
	assert.ok(visibleColumns(parseHidden('[]'), false).includes('qty'));
});

test('a stored choice survives a reload and drops unknown keys', () => {
	assert.deepEqual(parseHidden(JSON.stringify(['type', 'bogus'])), ['type']);
	assert.deepEqual(parseHidden('[]'), []);
});

test('hiding a column removes it', () => {
	assert.ok(!visibleColumns(['type'], false).includes('type'));
	assert.ok(visibleColumns([], false).includes('type'));
});

test('Adjusted is absent when nothing is adjusted, even if chosen', () => {
	assert.ok(!visibleColumns([], false).includes('adj'));
	assert.ok(visibleColumns([], true).includes('adj'));
	assert.ok(!visibleColumns(['adj'], true).includes('adj'));
});

test('anyAdjusted compares adjusted to Near Mint', () => {
	assert.equal(anyAdjusted([{ nm_unit: 1, market_unit: 1 }, { nm_unit: null, market_unit: null }]), false);
	assert.equal(anyAdjusted([{ nm_unit: 1, market_unit: 0.8 }]), true);
});

test('a phone starts with Name and Type only, the rest in the chooser', () => {
	const hidden = parseHidden(null, true);
	assert.deepEqual(visibleColumns(hidden, true), ['type']);
	assert.deepEqual(parseHidden('garbage', true), hidden);
	assert.deepEqual(parseHidden('[]', true), []);
});

test('Traits is its own column, hidden on a phone by default', () => {
	assert.ok(visibleColumns([], false).includes('traits'));
	assert.ok(!visibleColumns(parseHidden(null, true), true).includes('traits'));
});

test('a unit with an arena and three traits reads Type over arena, and one Traits line', async () => {
	const { typeLines, traitsLine } = await import('../src/lib/format.ts');
	assert.deepEqual(typeLines('Unit', '["Ground"]'), { type: 'Unit', arena: 'Ground' });
	assert.deepEqual(typeLines('Base', null), { type: 'Base', arena: '' });
	assert.equal(
		traitsLine('["Underworld","Droid","Bounty Hunter"]'),
		'Underworld · Droid · Bounty Hunter'
	);
});

test('the default header order begins Name, Type, Aspects, Cost, Traits', () => {
	const cols = visibleColumns(parseHidden(null), false);
	assert.deepEqual(cols.slice(0, 4), ['type', 'aspects', 'cost', 'traits']);
	const src = readFileSync(new URL('../src/routes/collection/+page.svelte', import.meta.url), 'utf8');
	const head = src.slice(src.indexOf('<thead>'), src.indexOf('</thead>'));
	const order = ['Name', 'Type', 'Aspects', 'Cost', 'Traits'].map((l) =>
		head.search(new RegExp(`'${l}'|<th>${l}</th>`))
	);
	assert.ok(order.every((i, n) => i >= 0 && (n === 0 || i > order[n - 1])), order.join());
});
