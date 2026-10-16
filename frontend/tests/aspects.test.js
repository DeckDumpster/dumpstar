import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';
import AspectIcon from '../src/lib/components/AspectIcon.svelte';
import { ASPECTS } from '../src/lib/aspects.ts';
import { markup } from './support/render.js';

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, '../..');
const setDir = join(repo, 'data/remedial/aspects');
/** @type {{ aspects: { name: string, file: string, source_url: string, retrieved: string, colour: string, sort_value: number }[] }} */
const manifest = JSON.parse(readFileSync(join(setDir, 'manifest.json'), 'utf8'));

test('a Vigilance/Heroism leader shows both official icons with their names as alt text', () => {
	const html = ['Vigilance', 'Heroism'].map((name) => markup(AspectIcon, { name })).join('');
	assert.match(html, /<img[^>]*src="\/aspects\/icon_vigilance_[^"]+\.png"[^>]*alt="Vigilance"/);
	assert.match(html, /<img[^>]*src="\/aspects\/icon_heroism_[^"]+\.png"[^>]*alt="Heroism"/);
	assert.match(html, /title="Heroism"/);
});

test('a name the manifest lacks falls back to its text', () => {
	assert.match(markup(AspectIcon, { name: 'Neutral' }), /Neutral/);
	assert.doesNotMatch(markup(AspectIcon, { name: 'Neutral' }), /<img/);
});

test('the manifest lists the aspects in the official order', () => {
	assert.deepEqual(ASPECTS.map((a) => a.sort_value), [1, 5, 10, 15, 20, 25]);
});

test('every manifest row has its file, source url, retrieval date, colour and sort value', () => {
	for (const a of manifest.aspects) {
		assert.ok(existsSync(join(setDir, a.file)), `${a.name}: ${a.file} missing`);
		assert.ok(statSync(join(setDir, a.file)).size > 0);
		assert.match(a.source_url, /^https:\/\/cdn\.starwarsunlimited\.com\/.+\.png$/);
		assert.match(a.retrieved, /^\d{4}-\d{2}-\d{2}$/);
		assert.match(a.colour, /^#[0-9a-f]{6}$/);
		assert.equal(typeof a.sort_value, 'number');
	}
	const pngs = readdirSync(setDir).filter((f) => f.endsWith('.png'));
	assert.deepEqual(pngs.sort(), manifest.aspects.map((a) => a.file).sort());
});

test('every aspect in the catalog has an icon and a manifest row', () => {
	/** @type {Set<string>} */
	const catalog = new Set();
	const fixtures = join(repo, 'fixtures/swu/official-sor-pages');
	for (const f of readdirSync(fixtures)) {
		const doc = JSON.parse(gunzipSync(readFileSync(join(fixtures, f))).toString());
		for (const c of doc.data ?? []) {
			for (const r of c.attributes?.aspects?.data ?? []) catalog.add(r.attributes.name);
		}
	}
	const fake = JSON.parse(readFileSync(join(repo, 'crates/dumpstar-catalog/fake/swu-fake.json'), 'utf8'));
	for (const c of fake.cards) for (const a of c.aspects) catalog.add(a);
	assert.ok(catalog.size >= 6, `catalog aspects: ${[...catalog]}`);
	const known = new Map(manifest.aspects.map((a) => [a.name, a]));
	for (const name of catalog) {
		const row = known.get(name);
		assert.ok(row, `${name} has no manifest row`);
		assert.ok(existsSync(join(setDir, row.file)), `${name} has no icon`);
	}
});

test('no source file outside the manifest loader names an aspect colour', () => {
	const colours = manifest.aspects.map((a) => a.colour.toLowerCase());
	const loader = join(here, '../src/lib/aspects.ts');
	/** @type {string[]} */
	const offenders = [];
	const walk = (/** @type {string} */ dir) => {
		for (const e of readdirSync(dir, { withFileTypes: true })) {
			const p = join(dir, e.name);
			if (e.isDirectory()) walk(p);
			else if (/\.(ts|js|svelte|css)$/.test(e.name) && p !== loader && !p.includes('/lib/kit/')) {
				const text = readFileSync(p, 'utf8').toLowerCase();
				for (const c of colours) if (text.includes(c)) offenders.push(`${relative(repo, p)}: ${c}`);
			}
		}
	};
	walk(join(here, '../src'));
	assert.deepEqual(offenders, []);
});

test('views show a card's aspects as printed, never re-sorted', () => {
	for (const f of ['src/routes/collection/+page.svelte', 'src/lib/components/CardDetailView.svelte']) {
		assert.doesNotMatch(readFileSync(join(here, '..', f), 'utf8'), /sortAspects/, f);
	}
	assert.equal(typeof (/** @type {any} */ (ASPECTS)).sortAspects, 'undefined');
});
