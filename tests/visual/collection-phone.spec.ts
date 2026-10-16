/**
 * /collection's list on a phone is deckdumpster's mobile table (sp-vztadt):
 * Name and Type only, names wrapping rather than truncating, nothing clipped.
 * Everything else is one tap away in the column chooser.
 *
 * The result is synthesised by intercepting the search endpoint, as
 * collection-window.spec.ts does, so the names can be long on purpose.
 */

import { test, expect } from '@playwright/test';
import { stabilize } from './stabilize';

test.use({ viewport: { width: 390, height: 844 } });

function row(i: number) {
	return {
		printing_id: `phone-${i}-normal`,
		card_id: `phone-${i}`,
		set_code: 'sor',
		set_name: 'Spark of Rebellion',
		set_symbol_url: null,
		number: String(i + 1),
		name: i % 3 === 0 ? `Darth Vader, Commanding the First Legion ${i}` : `Han Solo ${i}`,
		card_type: i % 2 ? 'Leader' : 'Unit',
		rarity: 'Common',
		market_price: 1.5,
		image_small: null,
		variant: 'normal',
		owned: true,
		owned_count: 1,
		copies: [
			{
				id: 1000 + i,
				condition: 'Near Mint',
				language: 'English',
				status: 'owned',
				graded: false,
				purchase_price: null,
				acquired_at: '2025-01-01',
				binder_id: null,
				deck_id: null
			}
		]
	};
}

test('the phone list is Name and Type, wrapped, with nothing clipped', async ({ page }) => {
	await stabilize(page);
	await page.route('**/api/collection/search*', (route) =>
		route.fulfill({
			json: {
				rows: Array.from({ length: 60 }, (_, i) => row(i)),
				total: 60,
				total_value: 90,
				limit: 60,
				offset: 0
			}
		})
	);
	await page.addInitScript(() => {
		localStorage.setItem('collection.view', 'table');
		localStorage.setItem('collection.sortKey', 'name');
		localStorage.setItem('collection.sortDir', 'asc');
		localStorage.removeItem('collection.hiddenColumns.phone');
		sessionStorage.clear();
	});
	await page.goto('/collection?q=set%3Asor&all=1');
	await page.locator('table.dd').waitFor();

	const heads = await page.locator('table.dd thead th').allInnerTexts();
	expect(heads.map((h) => h.replace(/[▲▼]/g, '').trim().toLowerCase())).toEqual(['name', 'type']);
	await expect(page.locator('table.dd thead th .caret')).toHaveCount(1);

	const report = await page.evaluate(() => {
		const clipped: string[] = [];
		for (const el of document.querySelectorAll('table.dd tbody td, table.dd thead th')) {
			const e = el as HTMLElement;
			if (e.scrollWidth > e.clientWidth + 1) clipped.push(`${e.className}:${e.textContent?.trim()}`);
		}
		const names = Array.from(document.querySelectorAll('table.dd .namebody')) as HTMLElement[];
		return {
			clipped,
			ellipsised: names.filter((n) => getComputedStyle(n).whiteSpace === 'nowrap').length,
			clippedNames: names.filter((n) => n.scrollHeight > n.clientHeight + 1).length,
			wrapped: names.filter((n) => n.getBoundingClientRect().height > 20).length,
			pageOverflow: document.documentElement.scrollWidth > window.innerWidth,
			tableOverflow:
				(document.querySelector('.tableScroll') as HTMLElement).scrollWidth >
				(document.querySelector('.tableScroll') as HTMLElement).clientWidth
		};
	});
	expect(report.clipped, 'no cell overflows its column').toEqual([]);
	expect(report.ellipsised, 'no name is on one nowrap line').toBe(0);
	expect(report.clippedNames, 'no name loses a line').toBe(0);
	expect(report.wrapped, 'a long name wraps').toBeGreaterThan(0);
	expect(report.tableOverflow, 'the table does not scroll sideways').toBe(false);
	expect(report.pageOverflow).toBe(false);
});
