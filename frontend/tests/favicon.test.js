/**
 * The browser tab is the one piece of chrome no screenshot covers.
 *
 * `static/favicon.svg` shipped as the stock SvelteKit scaffold mark for the
 * life of the project — a Svelte logo in the tab of a collection
 * tracker — and nothing failed, because no route renders it and the token
 * gates scan `src/` only. A favicon is a static asset fetched outside the
 * document: it cannot reference a custom property, so it is deliberately out
 * of the token layer's scope (see the note on pd-xqvg). Out of scope for the
 * ratchet is not the same as unowned, and this file is the ownership.
 *
 * Runs under Node's built-in runner — no test dependency:
 *   npm test        (frontend/)
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const frontendRoot = join(here, '..');
const faviconPath = join(frontendRoot, 'static/favicon.svg');
const tokensPath = join(frontendRoot, 'src/lib/styles/theme.css');

const favicon = readFileSync(faviconPath, 'utf8');

test('the favicon is the Dumpstar mark, not the scaffold logo', () => {
	// Both fingerprints of the scaffold asset, not the word "svelte" — the file
	// legitimately names DeathStar.svelte in a comment.
	for (const scaffold of [/svelte-logo/i, /#ff3e00/i]) {
		assert.ok(
			!scaffold.test(favicon),
			'static/favicon.svg is the stock SvelteKit logo again. The tab icon is the ' +
				'the second Death Star — same geometry as src/lib/components/DeathStar.svelte.'
		);
	}
	assert.match(
		favicon,
		/<title>Dumpstar<\/title>/,
		'the mark should name itself, so the next person to open the file knows what it is'
	);
});

test('the favicon is painted in the brand gold', () => {
	// It cannot spend a token, so it spells the reference step out. What this
	// asserts is that the two stay the same colour: a re-skin that edits the
	// reference block and forgets the tab icon fails here rather than shipping
	// a favicon in the old brand.
	const gold = readFileSync(tokensPath, 'utf8').match(
		/--pd-gold-500:\s*(#[0-9a-fA-F]{3,8})\s*;/
	);
	assert.ok(gold, 'theme.css no longer declares --pd-gold-500');
	assert.ok(
		favicon.toLowerCase().includes(gold[1].toLowerCase()),
		`static/favicon.svg does not carry ${gold[1]}, the gold-500 reference step. ` +
			'A favicon cannot resolve var(), so re-skinning the brand means editing ' +
			'this hex alongside the reference block in theme.css.'
	);
});

test('the favicon is well-formed XML', () => {
	// Served as image/svg+xml, so it is parsed as XML, not HTML. XML forbids
	// `--` inside a comment — and a token name like `--pd-gold-500` in an
	// explanatory comment is exactly how that gets in. The browser then drops
	// the whole image with no console error, which is unfalsifiable by eye if
	// the old icon is still cached. Caught for real while writing this file.
	const comments = [...favicon.matchAll(/<!--([\s\S]*?)-->/g)];
	for (const [, body] of comments) {
		assert.ok(
			!body.includes('--'),
			'an XML comment may not contain `--`; the browser silently drops the ' +
				'entire image. Write token names without the leading hyphens.'
		);
	}
	// The cheap structural checks that survive without an XML parser dependency.
	assert.match(favicon, /^<svg[^>]*\bxmlns="http:\/\/www\.w3\.org\/2000\/svg"/);
	assert.match(favicon, /<\/svg>\s*$/);
	assert.equal(
		(favicon.match(/</g) ?? []).length,
		(favicon.match(/>/g) ?? []).length,
		'unbalanced angle brackets'
	);
});

// The favicon is derived from the header's mark (src/lib/components/DeathStar.svelte)
// by scripts/sync-favicon.js, so the two cannot drift.
import { build, shapes } from '../scripts/sync-favicon.js';

test('the favicon set is exactly what the header art generates', () => {
	for (const [name, data] of Object.entries(build())) {
		const onDisk = readFileSync(join(frontendRoot, 'static', name));
		assert.ok(
			onDisk.equals(Buffer.from(data)),
			`static/${name} is stale against DeathStar.svelte; run node scripts/sync-favicon.js`
		);
	}
});

test('the favicon carries the header mark path data verbatim', () => {
	for (const e of shapes().els) {
		const attrsOf = e.src.replace(/\sfill="none"/, '');
		assert.ok(favicon.includes(attrsOf), `favicon.svg lacks the header's ${attrsOf}`);
	}
});

test('every page links the svg, 32px png and apple-touch icons', () => {
	// One SPA shell: every served HTML page renders through this layout's <svelte:head>.
	const layout = readFileSync(join(frontendRoot, 'src/routes/+layout.svelte'), 'utf8');
	for (const href of ['/favicon.svg', '/favicon-32.png', '/apple-touch-icon.png']) {
		assert.ok(layout.includes(`href="${href}"`), `+layout.svelte does not link ${href}`);
	}
	assert.match(layout, /rel="apple-touch-icon"[^>]*sizes="180x180"/);
	const png = (n) => readFileSync(join(frontendRoot, 'static', n));
	assert.equal(png('apple-touch-icon.png').readUInt32BE(16), 180);
	assert.equal(png('favicon-32.png').readUInt32BE(16), 32);
});
