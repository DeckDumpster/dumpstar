import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tileFace, tileCaption } from '../src/lib/tileFace.ts';

const front = 'https://img.test/vader-leader.png';
const back = 'https://img.test/vader-unit.png';

test('a leader tile shows the portrait back, unrotated', () => {
	const f = tileFace({ card_type: 'Leader', image_url: front, back_image_url: back });
	assert.equal(f.image, back);
	assert.equal(f.rotate, false);
});

test('a leader with no back turns its front', () => {
	const f = tileFace({ card_type: 'Leader', image_url: front, back_image_url: null });
	assert.equal(f.image, front);
	assert.equal(f.rotate, true);
});

test('a base tile carries the rotation', () => {
	const f = tileFace({ card_type: 'Base', image_url: front });
	assert.equal(f.image, front);
	assert.equal(f.rotate, true);
});

test('a unit is shown as printed', () => {
	const f = tileFace({ card_type: 'Unit', image_url: front, back_image_url: back });
	assert.deepEqual(f, { image: front, rotate: false, captioned: false });
});

test('a tile whose picture lacks the number names the card', () => {
	const leader = tileFace({ card_type: 'Leader', image_url: front, back_image_url: back });
	const base = tileFace({ card_type: 'Base', image_url: front });
	const unit = tileFace({ card_type: 'Unit', image_url: front });
	assert.equal(tileCaption('016', 'Darth Vader', leader), '#016 Darth Vader');
	assert.equal(tileCaption('020', 'Echo Base', base), '#020 Echo Base');
	assert.equal(tileCaption('100', 'Vanquish', unit), '#100');
});

test('turned landscape pictures rotate counter-clockwise, title on the left, in every view', async () => {
	const { readFileSync } = await import('node:fs');
	const src = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');
	for (const p of ['../src/routes/+page.svelte', '../src/routes/collection/+page.svelte']) {
		assert.match(src(p), /rotate\(-90deg\)/, p);
		assert.doesNotMatch(src(p), /rotate\(90deg\)/, p);
	}
	assert.match(src('../src/routes/browse/[set]/+page.svelte'), /\.card-tile\.rotate \.art img[^}]*rotate\(-90deg\)/);
});
