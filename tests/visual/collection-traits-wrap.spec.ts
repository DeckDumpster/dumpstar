/**
 * The Traits cell wraps at its "·" separators and never paints into the next
 * column (sp-k9obdq). Run for every project in playwright.config.
 */

import { test, expect } from '@playwright/test';
import { stabilize } from './stabilize';

test.use({ viewport: { width: 390, height: 844 } });

function row(i: number) {
	return {
		printing_id: `trait-${i}-normal`,
		card_id: `trait-${i}`,
		set_code: 'sor',
		set_name: 'Spark of Rebellion',
		set_symbol_url: null,
		number: String(i + 1),
		name: `Cad Bane ${i}`,
		card_type: 'Unit',
		rarity: 'Common',
		market_price: 1.5,
		image_small: null,
		variant: 'normal',
		aspects: '["Cunning","Aggression"]',
		traits: '["Underworld","Droid","Bounty Hunter"]',
		owned: true,
		owned_count: 1,
		copies: []
	};
}

test('trait links wrap after a separator and stay out of the Aspects column', async ({ page }) => {
	await stabilize(page);
	await page.route('**/api/collection/search*', (route) =>
		route.fulfill({
			json: {
				rows: Array.from({ length: 5 }, (_, i) => row(i)),
				total: 5,
				total_value: 8,
				limit: 60,
				offset: 0
			}
		})
	);
	await page.addInitScript(() => {
		localStorage.setItem('collection.view', 'table');
		localStorage.setItem('collection.hiddenColumns.phone', '[]');
		sessionStorage.clear();
	});
	await page.goto('/collection?q=set%3Asor');
	await page.locator('table.dd').waitFor();

	const m = await page.evaluate(() => {
		const tr = document.querySelector('table.dd tbody tr') as HTMLElement;
		const cell = tr.querySelector('td.traits') as HTMLElement;
		const aspects = tr.querySelector('td.aspects') as HTMLElement;
		const rects = Array.from(cell.querySelectorAll('.unit')).flatMap((u) =>
			Array.from(u.getClientRects()).map((r) => ({ left: r.left, right: r.right, top: r.top }))
		);
		return {
			contentRight: Math.max(...rects.map((r) => r.right)),
			cellRight: cell.getBoundingClientRect().right,
			aspectsLeft: aspects.getBoundingClientRect().left,
			lines: new Set(rects.map((r) => Math.round(r.top))).size
		};
	});
	expect(m.contentRight).toBeLessThanOrEqual(m.aspectsLeft);
	expect(m.contentRight).toBeLessThanOrEqual(m.cellRight + 1);
	expect(m.lines).toBeGreaterThan(1);
});
