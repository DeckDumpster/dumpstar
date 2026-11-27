import { test } from 'node:test';
import assert from 'node:assert/strict';
import { collectionCountLine } from '../src/lib/format.ts';

test('the count line names copies owned and distinct printings, with the value', () => {
	assert.equal(
		collectionCountLine(2914, 1635, 12.5),
		'2,914 copies owned · 1,635 distinct printings, $12.50'
	);
});

test('the count line is singular at one and shows no money without a value', () => {
	const line = collectionCountLine(1, 1, null);
	assert.equal(line, '1 copy owned · 1 distinct printing');
	assert.doesNotMatch(line, /\$/);
});

test('the count line never says "cards"', () => {
	/** @type {[number, number, number | null][]} */
	const cases = [
		[2914, 1635, 12.5],
		[1, 1, null],
		[0, 0, null]
	];
	for (const args of cases) {
		assert.doesNotMatch(collectionCountLine(...args), /cards/);
	}
});
