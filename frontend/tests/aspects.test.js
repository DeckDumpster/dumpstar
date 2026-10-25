import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';
import AspectIcon from '../src/lib/components/AspectIcon.svelte';
import { ASPECTS, aspectOf } from '../src/lib/aspects.ts';
import { markup } from './support/render.js';

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, '../..');
const CDN = 'https://cdn.starwarsunlimited.com/';

test('a Vigilance/Heroism leader shows both official icons with their names as alt text', () => {
	const html = ['Vigilance', 'Heroism'].map((name) => markup(AspectIcon, { name })).join('');
	assert.match(html, /<img[^>]*src="https:\/\/cdn\.starwarsunlimited\.com\/+xxxsmall_SWH_Aspects_Vigilance_[^"]+\.png"[^>]*alt="Vigilance"/);
	assert.match(html, /<img[^>]*src="https:\/\/cdn\.starwarsunlimited\.com\/+xxxsmall_SWH_Aspects_Heroism_[^"]+\.png"[^>]*alt="Heroism"/);
	assert.match(html, /title="Heroism"/);
});

test('every aspect icon src and srcset point at the publisher CDN', () => {
	assert.equal(ASPECTS.length, 6);
	for (const a of ASPECTS) {
		const html = markup(AspectIcon, { name: a.name });
		const src = html.match(/ src="([^"]+)"/)?.[1] ?? '';
		const srcset = html.match(/ srcset="([^"]+)"/)?.[1] ?? '';
		assert.ok(src.startsWith(CDN), `${a.name}: ${src}`);
		const urls = srcset.split(',').map((e) => e.trim().split(' ')[0]);
		assert.equal(urls.length, 3, a.name);
		for (const u of urls) assert.ok(u.startsWith(CDN) && u.includes(a.base), `${a.name}: ${u}`);
		assert.match(srcset, /149w.*199w.*248w/);
	}
});

test('no aspect PNG is in the repository or served by the app', () => {
	assert.ok(!existsSync(join(repo, 'data/remedial/aspects')));
	assert.ok(!existsSync(join(here, '../static/aspects')));
	assert.ok(!existsSync(join(here, '../scripts/sync-aspects.js')));
	assert.doesNotMatch(readFileSync(join(repo, 'crates/dumpstar-server/src/lib.rs'), 'utf8'), /"\/aspects"/);
});

test('a name the set lacks falls back to its text', () => {
	assert.match(markup(AspectIcon, { name: 'Neutral' }), /Neutral/);
	assert.doesNotMatch(markup(AspectIcon, { name: 'Neutral' }), /<img/);
});

test('the set lists the aspects in the official order', () => {
	assert.deepEqual(ASPECTS.map((a) => a.sort_value), [1, 5, 10, 15, 20, 25]);
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
	for (const name of catalog) assert.ok(aspectOf(name), `${name} has no aspect row`);
});

test('no source file outside the manifest loader names an aspect colour', () => {
	const colours = ASPECTS.map((a) => a.colour.toLowerCase());
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

test('views show the aspects of a card as printed, never re-sorted', () => {
	for (const f of ['src/routes/collection/+page.svelte', 'src/lib/components/CardDetailView.svelte']) {
		assert.doesNotMatch(readFileSync(join(here, '..', f), 'utf8'), /sortAspects/, f);
	}
	assert.equal(typeof (/** @type {any} */ (ASPECTS)).sortAspects, 'undefined');
});
