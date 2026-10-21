/**
 * Hovering a base thumbnail on the home page does not move it (sp-irs3cj).
 *
 * The thumbnail hover lift (`translateY(-2px)`) replaced the turned image's own
 * `translate(-50%, -50%) rotate(-90deg)`, so a base animated from rotated and
 * centred to unrotated and offset: a quarter turn, clipped out of its box.
 * Asserts the image's computed transform is the same before and during hover
 * and that its box stays inside the tile. Run for every project in
 * playwright.config.
 */

import { test, expect } from '@playwright/test';
import { stabilize } from './stabilize';

// A 7:5 landscape picture, as a base is: turned, it fills the 245:342 tile.
const LANDSCAPE =
	'<svg xmlns="http://www.w3.org/2000/svg" width="700" height="500" viewBox="0 0 700 500"><rect width="700" height="500" fill="#456"/><rect x="20" y="20" width="660" height="460" fill="none" stroke="#fff" stroke-width="10"/></svg>';

const BASE = {
	id: 1,
	printing_id: 'sor-020-normal',
	condition: 'Near Mint',
	language: 'en',
	purchase_price: null,
	sale_price: null,
	acquired_at: '2026-01-14',
	source: 'test',
	notes: null,
	status: 'owned',
	graded: false,
	binder_id: null,
	deck_id: null,
	variant: 'normal',
	finish: 'Normal',
	card_id: 'echo-base',
	set_code: 'sor',
	set_name: 'Spark of Rebellion',
	number: '020',
	name: 'Echo Base',
	subtitle: null,
	rarity: 'Common',
	card_type: 'Base',
	image_url: 'https://cards.example/echo-base.png',
	back_image_url: null
};

test('hovering a base on the home page keeps it turned and inside its tile', async ({ page }) => {
	await stabilize(page);
	// The page's other reads (value history, sets, unresolved) are all lists.
	await page.route('**/api/**', (route) => route.fulfill({ json: [] }));
	await page.route('https://cards.example/**', (route) =>
		route.fulfill({ status: 200, contentType: 'image/svg+xml', body: LANDSCAPE })
	);
	await page.route('**/api/batches**', (route) => {
		const path = new URL(route.request().url()).pathname;
		if (path.endsWith('/api/batches')) {
			return route.fulfill({
				json: [
					{
						id: 1,
						batch_type: 'manual',
						name: null,
						notes: null,
						order_id: null,
						binder_id: null,
						created_at: '2026-01-14T00:00:00Z',
						card_count: 1
					}
				]
			});
		}
		return route.fulfill({ json: { batch: null, cards: [BASE] } });
	});

	await page.goto('/', { waitUntil: 'domcontentloaded' });
	const img = page.locator('.thumbs .face.turned img').first();
	const tile = page.locator('.thumbs .face.turned').first();
	await expect(img).toBeVisible();
	await expect
		.poll(() => img.evaluate((el) => (el as HTMLImageElement).complete && el.naturalWidth > 0))
		.toBe(true);

	const transform = () => img.evaluate((el) => getComputedStyle(el).transform);
	const before = await transform();
	expect(before).not.toBe('none');

	await img.hover();
	await page.waitForTimeout(400);
	expect(await transform()).toBe(before);

	const [box, tileBox] = [await img.boundingBox(), await tile.boundingBox()];
	expect(box).not.toBeNull();
	expect(tileBox).not.toBeNull();
	const eps = 1.5;
	expect(box!.x).toBeGreaterThanOrEqual(tileBox!.x - eps);
	expect(box!.y).toBeGreaterThanOrEqual(tileBox!.y - eps);
	expect(box!.x + box!.width).toBeLessThanOrEqual(tileBox!.x + tileBox!.width + eps);
	expect(box!.y + box!.height).toBeLessThanOrEqual(tileBox!.y + tileBox!.height + eps);
});
