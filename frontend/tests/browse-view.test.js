import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseView, splitLandscape, clampCols, MAX_COLS, BROWSE_ROWS, pageAfterResize } from '$lib/browseView.js';

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

test('an out-of-range cols clamps to the maximum; junk falls back to 3', () => {
	assert.equal(clampCols(10), MAX_COLS);
	assert.equal(clampCols(9), MAX_COLS);
	assert.equal(clampCols(0), 1);
	assert.equal(clampCols(NaN), 3);
	assert.equal(clampCols(MAX_COLS + 1), MAX_COLS);
});

test('the frontend page size maximum equals the backend BINDER_MAX_LAYOUT', () => {
	const src = readFileSync(
		new URL('../../crates/dumpstar-db/src/binder.rs', import.meta.url),
		'utf8'
	);
	const m = src.match(/pub const BINDER_MAX_LAYOUT: i64 = (\d+);/);
	assert.ok(m, 'BINDER_MAX_LAYOUT not found in binder.rs');
	assert.equal(MAX_COLS * BROWSE_ROWS, Number(m[1]));
});

test('pageAfterResize keeps the first visible card on screen', () => {
	// page 4 at 3 cols (9/page) starts at card 27; at 4 cols (12/page) that is page 3
	assert.equal(pageAfterResize(4, 9, 12), 3);
	assert.equal(pageAfterResize(4, 9, 6), 5);
	assert.equal(pageAfterResize(1, 9, 30), 1);
	assert.notEqual(pageAfterResize(4, 9, 12), 1);
});

test('card-view footer wraps its pips onto their own row, at a 24px tap target', () => {
	const src = readFileSync(new URL('../src/routes/browse/[set]/+page.svelte', import.meta.url), 'utf8');
	const meta = src.match(/\n\t\.meta \{[^}]*\}/)[0];
	assert.match(meta, /flex-wrap:\s*wrap/);
	assert.match(src.match(/\.meta \.vchips \{[^}]*\}/)[0], /flex:\s*1 0 100%/);
	assert.match(src.match(/\n\t\.vchip \{[^}]*\}/)[0], /width:\s*24px;\s*height:\s*24px/);
});
