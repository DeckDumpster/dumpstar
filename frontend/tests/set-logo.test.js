import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { markup } from './support/render.js';
import SetLogo from '../src/lib/components/SetLogo.svelte';

// Server render has no fetch resolved yet, so the component shows nothing
// rather than a broken image; the wiring is asserted on the sources.
test('SetLogo renders no <img> until the set has a retrieved logo', () => {
	assert.doesNotMatch(markup(SetLogo, { code: 'SOR', name: 'Spark of Rebellion' }), /<img/);
});

test('a set with a logo renders only the image, named by alt and title; one without shows its code', () => {
	const withLogo = markup(SetLogo, { code: 'SOR', name: 'Spark of Rebellion', logo: '/art/sor.png' });
	assert.match(withLogo, /<img[^>]*alt="Spark of Rebellion"/);
	assert.match(withLogo, /title="Spark of Rebellion"/);
	assert.doesNotMatch(withLogo, /SOR|<span/);
	assert.match(markup(SetLogo, { code: 'sor', name: 'Spark of Rebellion' }), /<span[^>]*>SOR<\/span>/);
});

test('the collection Set cell, the card page and the /browse tile each show the set logo', () => {
	const read = (/** @type {string} */ f) => readFileSync(new URL(f, import.meta.url), 'utf8');
	const collection = read('../src/routes/collection/+page.svelte');
	assert.equal((collection.match(/<SetLogo code=\{a\.set_code\}/g) ?? []).length, 2);
	assert.match(read('../src/lib/components/CardDetailView.svelte'), /<SetLogo code=\{card\.set_code\}/);
	assert.match(read('../src/routes/browse/+page.svelte'), /<img class="logo" src=\{set\.logo_url\}/);
	assert.match(read('../src/lib/components/SetLogo.svelte'), /<img class="setmark" \{src\}/);
});

test('the set page header fits the logo to a fixed box beside the crop, never by intrinsic size', () => {
	const src = readFileSync(new URL('../src/routes/browse/[set]/+page.svelte', import.meta.url), 'utf8');
	assert.match(src, /<div class="logobox">\s*<img class="setlogo"/);
	const css = (/** @type {string} */ sel) => src.match(new RegExp(`\\n\\t${sel} \\{([^}]*)\\}`))?.[1] ?? '';
	const box = css('\\.logobox');
	assert.match(box, /min-width: 0/);
	assert.match(box, /height: /);
	const logo = css('\\.setlogo');
	assert.match(logo, /width: 100%/);
	assert.match(logo, /height: 100%/);
	assert.match(logo, /object-fit: contain/);
	assert.match(css('\\.setcrop'), /max-width: 40%/);
});
