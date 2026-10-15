import test from 'node:test';
import assert from 'node:assert/strict';
import { parseView, splitLandscape } from '$lib/browseView.js';

test('card view is the default; only view=binder selects binder', () => {
	assert.equal(parseView(null), 'card');
	assert.equal(parseView(undefined), 'card');
	assert.equal(parseView('card'), 'card');
	assert.equal(parseView('bogus'), 'card');
	assert.equal(parseView('binder'), 'binder');
});

test('splitLandscape keeps order within each group', () => {
	const s = [
		{ n: 'L1', landscape: true },
		{ n: 'B1', landscape: true },
		{ n: 'U1', landscape: false },
		{ n: 'U2', landscape: false }
	];
	const { landscape, portrait } = splitLandscape(s);
	assert.deepEqual(landscape.map((x) => x.n), ['L1', 'B1']);
	assert.deepEqual(portrait.map((x) => x.n), ['U1', 'U2']);
});
