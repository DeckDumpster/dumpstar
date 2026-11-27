/**
 * A sort arrow sits inside its own header cell and never paints over the next
 * header (sp-b3gtjs). Run for every project in playwright.config.
 */

import { test, expect } from '@playwright/test';
import { stabilize } from './stabilize';

test.use({ viewport: { width: 390, height: 844 } });

const KEYS = ['qty', 'name', 'type', 'aspects', 'cost', 'rarity', 'set', 'number', 'price'];

function row(i: number) {
	return {
		printing_id: `arrow-${i}-normal`,
		card_id: `arrow-${i}`,
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
		traits: '["Underworld"]',
		owned: true,
		owned_count: 1,
		copies: []
	};
}

for (const dir of ['asc', 'desc'] as const) {
	test(`sort arrow stays inside its header cell (${dir})`, async ({ page }) => {
		await stabilize(page);
		await page.route('**/api/collection/search*', (route) =>
			route.fulfill({
				json: { rows: [row(0), row(1)], total: 2, total_value: 3, limit: 60, offset: 0 }
			})
		);
		for (const key of KEYS) {
			await page.addInitScript(
				([k, d]) => {
					localStorage.setItem('collection.view', 'table');
					localStorage.setItem('collection.hiddenColumns.phone', '[]');
					localStorage.setItem('collection.sortKey', k);
					localStorage.setItem('collection.sortDir', d);
				},
				[key, dir]
			);
			await page.goto('/collection?q=set%3Asor');
			await page.locator('table.dd').waitFor();
			const m = await page.evaluate(() => {
				const caret = document.querySelector('table.dd th.sortable .caret') as HTMLElement | null;
				if (!caret) return null;
				const th = caret.closest('th') as HTMLElement;
				const c = caret.getBoundingClientRect();
				const t = th.getBoundingClientRect();
				return { cl: c.left, cr: c.right, ct: c.top, cb: c.bottom, tl: t.left, tr: t.right, tt: t.top, tb: t.bottom };
			});
			expect(m, `caret rendered for ${key}`).not.toBeNull();
			expect(m!.cl, key).toBeGreaterThanOrEqual(m!.tl - 0.5);
			expect(m!.cr, key).toBeLessThanOrEqual(m!.tr + 0.5);
			expect(m!.ct, key).toBeGreaterThanOrEqual(m!.tt - 0.5);
			expect(m!.cb, key).toBeLessThanOrEqual(m!.tb + 0.5);
		}
	});
}
