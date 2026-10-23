/**
 * /collection's header controls are deckdumpster's, to the pixel (sp-0d7hvi).
 *
 * The search input, the list/grid toggle, the options button, the help (?)
 * button and the columns button were each a few pixels larger here than on
 * deck. The reference is not a recorded number: fixtures/deck-toolbar.html is
 * deck's own header markup and stylesheets, verbatim, and this spec measures
 * it in the same browser as dumpstar's page — so a font difference between
 * machines moves both sides together and a failure means the box changed.
 *
 * Runs once per project in playwright.config, at 390px (the phone) and at
 * 1280px.
 */

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from '@playwright/test';
import { stabilize } from './stabilize';

const deckHtml = readFileSync(
	join(dirname(fileURLToPath(import.meta.url)), 'fixtures/deck-toolbar.html'),
	'utf8'
);

const CONTROLS = {
	search: { deck: '#search-input', star: '[data-testid="search-input"]' },
	'list toggle': { deck: '#view-table-btn', star: '[data-testid="view-table"]' },
	'grid toggle': { deck: '#view-grid-btn', star: '[data-testid="view-grid"]' },
	options: { deck: '#more-menu-btn', star: '[data-testid="more-menu"]' },
	help: { deck: '#syntax-help-btn', star: '[data-testid="search-help-link"]' },
	columns: { deck: '#cols-btn', star: '[data-testid="columns-button"]' }
} as const;

type Box = { width: number; height: number };

async function measure(page: Page, which: 'deck' | 'star'): Promise<Record<string, Box>> {
	const selectors = Object.fromEntries(Object.entries(CONTROLS).map(([k, v]) => [k, v[which]]));
	return page.evaluate((sel) => {
		const out: Record<string, { width: number; height: number }> = {};
		for (const [name, q] of Object.entries(sel)) {
			const r = (document.querySelector(q) as HTMLElement).getBoundingClientRect();
			out[name] = { width: r.width, height: r.height };
		}
		return out;
	}, selectors);
}

function row(i: number) {
	return {
		printing_id: `tb-${i}-normal`,
		card_id: `tb-${i}`,
		set_code: 'sor',
		set_name: 'Spark of Rebellion',
		set_symbol_url: null,
		number: String(i + 1),
		name: `Han Solo ${i}`,
		card_type: 'Unit',
		rarity: 'Common',
		market_price: 1.5,
		image_small: null,
		variant: 'normal',
		owned: true,
		owned_count: 1,
		copies: []
	};
}

for (const width of [390, 1280]) {
	test(`the header controls match deck's boxes within 1px at ${width}px`, async ({ page }) => {
		await page.setViewportSize({ width, height: 844 });

		await page.setContent(deckHtml);
		const deck = await measure(page, 'deck');

		await stabilize(page);
		await page.route('**/api/collection/search*', (route) =>
			route.fulfill({
				json: {
					rows: Array.from({ length: 20 }, (_, i) => row(i)),
					total: 20,
					total_value: 30,
					limit: 20,
					offset: 0
				}
			})
		);
		await page.addInitScript(() => {
			localStorage.setItem('collection.view', 'table');
			sessionStorage.clear();
		});
		await page.goto('/collection?q=set%3Asor');
		await page.locator('[data-testid="columns-button"]').waitFor();
		const star = await measure(page, 'star');

		for (const name of Object.keys(CONTROLS)) {
			const tag = `${name}: deck ${deck[name].width}x${deck[name].height}, dumpstar ${star[name].width}x${star[name].height}`;
			expect(Math.abs(star[name].height - deck[name].height), `height — ${tag}`).toBeLessThanOrEqual(1);
			expect(Math.abs(star[name].width - deck[name].width), `width — ${tag}`).toBeLessThanOrEqual(1);
		}
	});
}
