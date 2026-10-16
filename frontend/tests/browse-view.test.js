import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseView, splitLandscape, bandRuns, clampCols, MAX_COLS, BROWSE_ROWS, pageAfterResize } from '$lib/browseView.js';

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
	/** @param {RegExp} re */
	const rule = (re) => {
		const m = src.match(re);
		assert.ok(m, `no CSS rule matches ${re}`);
		return m[0];
	};
	assert.match(rule(/\n\t\.meta \{[^}]*\}/), /flex-wrap:\s*wrap/);
	assert.match(rule(/\.meta \.vchips \{[^}]*\}/), /flex:\s*1 0 100%/);
	assert.match(rule(/\n\t\.vchip \{[^}]*\}/), /width:\s*24px;\s*height:\s*24px/);
});

// HMW-shaped set: Leaders #1-18, Bases #19-34, units 35-272, Hyperspace
// Leaders #273-290, Hyperspace Bases #291-306, units to 782, Showcase
// Leaders #783-800.
/** @type {{ number: string, card_type: string, landscape: boolean }[]} */
const hmw = [];
/**
 * @param {number} from
 * @param {number} to
 * @param {string} card_type
 */
const add = (from, to, card_type) => {
	for (let n = from; n <= to; n++)
		hmw.push({ number: String(n), card_type, landscape: card_type === 'Leader' || card_type === 'Base' });
};
add(1, 18, 'Leader');
add(19, 34, 'Base');
add(35, 272, 'Unit');
add(273, 290, 'Leader');
add(291, 306, 'Base');
add(307, 782, 'Unit');
add(783, 800, 'Leader');
/**
 * @param {number} cols
 * @param {number} p
 * @param {boolean} [desc]
 */
const pageOf = (cols, p, desc = false) => {
	const all = desc ? [...hmw].reverse() : hmw;
	const size = cols * BROWSE_ROWS;
	return all.slice((p - 1) * size, p * size);
};
/** @param {{ kind: string, slots: { number: string }[] }[]} runs */
const shape = (runs) => runs.map((r) => `${r.kind}:${r.slots[0].number}-${r.slots[r.slots.length - 1].number}`);

test('number sort at cols=6: page 1 is one Leaders band, page 2 Bases then portrait #35, #36', () => {
	assert.deepEqual(shape(bandRuns(pageOf(6, 1))), ['leaders:1-18']);
	assert.deepEqual(shape(bandRuns(pageOf(6, 2))), ['bases:19-34', 'portrait:35-36']);
});

test('number sort: the page holding #273 shows portrait cards, then a Leaders band from #273', () => {
	const p = Math.floor(272 / 18) + 1;
	const runs = bandRuns(pageOf(6, p));
	assert.deepEqual(shape(runs), ['portrait:271-272', 'leaders:273-288']);
});

test('number sort desc: runs follow the descending sequence, bands labelled by kind', () => {
	const runs = bandRuns(pageOf(6, 1, true));
	assert.deepEqual(shape(runs), ['leaders:800-783']);
});

test('bandRuns never reorders: flattening the runs gives the input back', () => {
	for (const p of [1, 2, 3, 16, 17, 45]) {
		const slots = pageOf(6, p);
		assert.deepEqual(bandRuns(slots).flatMap((r) => r.slots), slots);
	}
});

test('a page that begins mid-band still opens with that band', () => {
	assert.deepEqual(shape(bandRuns(hmw.slice(10, 28))), ['leaders:11-18', 'bases:19-28']);
});

test('the set page renders bands only under the number sort', () => {
	const src = readFileSync(new URL('../src/routes/browse/[set]/+page.svelte', import.meta.url), 'utf8');
	assert.match(src, /sortKey === 'number'\s*\?\s*\{ landscape: \[\], portrait: \[\] \}/);
	assert.match(src, /data-testid="landscape-band"/);
});
