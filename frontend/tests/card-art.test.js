import { test } from 'node:test';
import assert from 'node:assert/strict';
import { markup } from './support/render.js';
import CardArt from '../src/lib/components/CardArt.svelte';

const front = 'https://img.test/luke-leader.png';
const back = 'https://img.test/luke-unit.png';

test('a leader shows the flip button, on its leader side first', () => {
	const html = markup(CardArt, { front, back, name: 'Luke Skywalker' });
	assert.match(html, /class="flip/);
	assert.match(html, /aria-label="Show deployed side"/);
	assert.ok(html.includes(`src="${front}"`));
	assert.ok(!html.includes(back));
});

test('flipped shows the deployed side and offers the way back', () => {
	const html = markup(CardArt, { front, back, name: 'Luke Skywalker', flipped: true });
	assert.ok(html.includes(`src="${back}"`));
	assert.ok(!html.includes(front));
	assert.match(html, /aria-label="Show leader side"/);
});

test('a one-sided card shows no flip button', () => {
	const html = markup(CardArt, { front, back: null, name: 'Vanquish' });
	assert.doesNotMatch(html, /class="flip/);
	assert.ok(html.includes(`src="${front}"`));
});
