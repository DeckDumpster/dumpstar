/**
 * Card text with the official inline icons (sp-el3uaa): the styled text is
 * reduced to the icon/strong/em subset, every icon comes from the repository's
 * asset set with alt text, and nothing points at the publisher's host.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { cardTextHtml } from '../src/lib/cardtext.js';

const manifestPath = new URL('../../data/remedial/inline-icons/manifest.json', import.meta.url);
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));

const LUKE =
	'<p><strong>Action [</strong><img src="https://ffg-prod-uploads.s3.amazonaws.com/icon_cost_1_6878a6fe98.png" alt="icon_cost_1.png"><strong>, </strong><img src="https://ffg-prod-uploads.s3.amazonaws.com/icon_exhaust_0c450d09d4.png" alt="icon_exhaust.png"><strong>]:</strong> Give a Shield token.&nbsp;</p>';

test('exhaust and cost icons render from the repo asset set with alt text', () => {
	const html = cardTextHtml(LUKE);
	assert.match(html, /<img class="gameIcon" src="\/icons\/icon_cost_1_6878a6fe98\.png" alt="1 resource"/);
	assert.match(html, /<img class="gameIcon" src="\/icons\/icon_exhaust_0c450d09d4\.png" alt="Exhaust"/);
	assert.ok(!html.includes('amazonaws'));
	assert.ok(html.includes('<strong>Action [</strong>'));
});

test('a doubled slash in the publisher URL still finds the unique icon', () => {
	const html = cardTextHtml(
		'<p><img src="https://ffg-prod-uploads.s3.amazonaws.com//icon_unique_white_a08b37532f.png" alt="x"> Hi</p>'
	);
	assert.match(html, /src="\/icons\/icon_unique_white_a08b37532f\.png" alt="Unique"/);
});

test('anything outside the subset is dropped, never rendered', () => {
	const html = cardTextHtml(
		'<p onclick="x()">a<script>alert(1)</script><a href="javascript:x">l</a><img src="https://evil.test/x.png" onerror="y()"><img src="/icons/not-in-manifest.png"> <em>b</em> 1 < 2 & 3</p>'
	);
	assert.ok(!/<script|<a |<img|onclick|onerror|href|evil/.test(html));
	assert.ok(html.includes('<em>b</em>'));
	assert.ok(html.includes('1 &lt; 2 &amp; 3'));
	assert.ok(html.startsWith('<p>'));
});

test('unbalanced tags are closed', () => {
	assert.equal(cardTextHtml('<p><strong>a'), '<p><strong>a</strong></p>');
	assert.equal(cardTextHtml('a</p>'), 'a');
});

test('empty text renders nothing', () => {
	assert.equal(cardTextHtml(null), '');
	assert.equal(cardTextHtml(''), '');
});

test('every manifest entry has its file, source URL and retrieval date', () => {
	for (const i of manifest.icons) {
		assert.ok(existsSync(new URL(i.file, manifestPath)), i.file);
		assert.ok(i.alt && i.retrieved && i.source_url.endsWith(i.file), i.file);
	}
});
