import { test } from 'node:test';
import assert from 'node:assert/strict';
import { markup } from './support/render.js';
import PreconList from '../src/lib/components/PreconList.svelte';
import { tryAddPrecon } from '../src/lib/preconAdd.ts';
import { api } from '../src/lib/api.ts';

/** @param {string} precon_id @param {string} set_name @param {string} name @param {number} card_count */
const precon = (precon_id, set_name, name, card_count, extra = {}) => ({
	precon_id,
	product_id: 1,
	name,
	deck: null,
	set_code: 'X',
	set_name,
	kind: 'spotlight',
	card_count,
	leader_image: null,
	...extra
});

// Deliberately not alphabetical: the server's order (newest set first) is kept.
const precons = [
	precon('jtl-boba-fett', 'Jump to Lightspeed', 'Spotlight Deck: Boba Fett', 52, {
		leader_image: '/img/boba.webp'
	}),
	precon('jtl-han-solo', 'Jump to Lightspeed', 'Spotlight Deck: Han Solo', 51),
	precon('sor-luke', 'Spark of Rebellion', 'Two-Player Starter', 50, { deck: 'Luke Skywalker' })
];

const noop = () => {};
const props = { precons, busy: false, repeat: null, onAdd: noop, onConfirm: noop, onCancel: noop };
const ADD = 'Add to collection and create deck';
/** @param {string} html */
const text = (html) => html.replace(/<!--.*?-->/g, '').replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ');
/** @param {string} s @param {string} needle */
const count = (s, needle) => s.split(needle).length - 1;

test('rows are grouped by set in the order given', () => {
	const html = markup(PreconList, props);
	const heads = [...html.matchAll(/<h2[^>]*>([\s\S]*?)<\/h2>/g)].map((m) => text(m[1]).trim());
	assert.deepEqual(heads, ['Jump to Lightspeed', 'Spark of Rebellion']);
	const t = text(html);
	const at = ['Boba Fett', 'Han Solo', 'Spark of Rebellion', 'Two-Player Starter'].map((n) => t.indexOf(n));
	assert.deepEqual(at, [...at].sort((a, b) => a - b));
});

test('each row shows leader art, name, card count and its add button', () => {
	const html = markup(PreconList, props);
	const rows = html.match(/<li\b[\s\S]*?<\/li>/g) ?? [];
	assert.equal(rows.length, 3);
	for (const [i, want] of [
		['Spotlight Deck: Boba Fett', '52 cards'],
		['Spotlight Deck: Han Solo', '51 cards'],
		['Two-Player Starter (Luke Skywalker)', '50 cards']
	].entries()) {
		const t = text(rows[i]);
		for (const s of want) assert.ok(t.includes(s), `row ${i} lacks ${s}: ${t}`);
		assert.equal(count(t, ADD), 1);
	}
	assert.match(rows[0], /<img[^>]*src="\/img\/boba\.webp"/);
});

test('a repeat add answered 409 shows the server message and a confirm', async () => {
	const realFetch = globalThis.fetch;
	/** @type {unknown[]} */
	const bodies = [];
	globalThis.fetch = async (url, init) => {
		bodies.push([String(url), JSON.parse(String(init?.body))]);
		return new Response("conflict: already added as deck 'Spotlight Deck: Boba Fett' (id 4); add again to get a second deck and second copies", { status: 409 });
	};
	/** @type {Awaited<ReturnType<typeof tryAddPrecon>> | undefined} */
	let outcome;
	try {
		outcome = await tryAddPrecon(api.addPrecon, 'jtl-boba-fett', false);
	} finally {
		globalThis.fetch = realFetch;
	}
	assert.deepEqual(bodies, [['/api/precons/jtl-boba-fett/add', { confirm_repeat: false }]]);
	assert.ok(outcome && 'refusal' in outcome);
	assert.match(outcome.refusal, /^already added as deck 'Spotlight Deck: Boba Fett'/);

	const message = outcome.refusal ?? '';
	const html = markup(PreconList, { ...props, repeat: { id: 'jtl-boba-fett', message } });
	const rows = html.match(/<li\b[\s\S]*?<\/li>/g) ?? [];
	assert.ok(text(String(rows[0])).includes('already added as deck'));
	assert.match(text(String(rows[0])), /Add a second copy/);
	assert.ok(!text(String(rows[1])).includes('already added'));
});

test('confirming re-sends with confirm_repeat and returns the new deck', async () => {
	/** @type {unknown[]} */
	const calls = [];
	const outcome = await tryAddPrecon(
		async (...args) => {
			calls.push(args);
			return /** @type {any} */ ({ id: 9 });
		},
		'jtl-boba-fett',
		true
	);
	assert.deepEqual(calls, [['jtl-boba-fett', true]]);
	assert.deepEqual(outcome, { deck: { id: 9 } });
});

test('any other failure is thrown for the error line', async () => {
	const boom = new Error('500 boom');
	await assert.rejects(tryAddPrecon(async () => { throw boom; }, 'x', false), boom);
});
