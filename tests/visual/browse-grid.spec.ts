/**
 * The /browse set-tile grid wraps to what fits (sp-uya3ot).
 *
 * On a ~390px phone the grid showed 7 tiles per row and overflowed the
 * viewport: the Jump-to chip row's nowrap chips sized a bare `1fr` track far
 * wider than the screen, and `repeat(auto-fill, minmax(200px, 1fr))` then
 * dutifully fitted columns into that. The tile grid is pokedumpster's
 * (frontend/src/routes/browse/+page.svelte, `.grid`, lines 481-487 at
 * origin/master): min tile width 200px, column-gap var(--space-4).
 *
 * Asserts, at each width, that tiles per row equal what auto-fill yields for
 * the container (floor((width + gap) / (200 + gap)), the CSS formula, which
 * is floor(width / 200) when the gap is zero) and that the page does not
 * scroll sideways. Runs in the browser tier (deploy/ci.sh step 10, the
 * `container` tier), against the fixture instance.
 */

import { test, expect } from '@playwright/test';
import { stabilize, settle } from './stabilize';

/** pokedumpster's minmax() floor for a set tile. */
const MIN_TILE = 200;

const VIEWPORTS = [
	{ width: 390, height: 844 },
	{ width: 768, height: 1024 },
	{ width: 1280, height: 800 }
];

for (const viewport of VIEWPORTS) {
	test(`/browse set grid wraps to what fits at ${viewport.width}px`, async ({ page }) => {
		await page.setViewportSize(viewport);
		await stabilize(page);
		const response = await page.goto('/browse', { waitUntil: 'domcontentloaded' });
		expect(response?.status() ?? 0).toBeLessThan(400);
		await settle(page, '.grid .tile');

		const report = await page.evaluate(() => {
			const doc = document.documentElement;
			const grids = [...document.querySelectorAll<HTMLElement>('.grid')].map((grid) => {
				const style = getComputedStyle(grid);
				const inner =
					grid.clientWidth -
					parseFloat(style.paddingLeft) -
					parseFloat(style.paddingRight);
				const lefts = new Set(
					[...grid.querySelectorAll<HTMLElement>('.tile')].map((t) =>
						Math.round(t.getBoundingClientRect().left)
					)
				);
				return {
					inner,
					gap: parseFloat(style.columnGap) || 0,
					perRow: lefts.size,
					tiles: grid.querySelectorAll('.tile').length,
					overflow: grid.scrollWidth - grid.clientWidth
				};
			});
			return { scrollWidth: doc.scrollWidth, clientWidth: doc.clientWidth, grids };
		});

		expect(report.grids.length).toBeGreaterThan(0);
		expect(report.scrollWidth, 'page scrolls sideways').toBeLessThanOrEqual(viewport.width);
		for (const g of report.grids) {
			const fits = Math.max(1, Math.floor((g.inner + g.gap) / (MIN_TILE + g.gap)));
			expect(g.perRow, `tiles per row in a ${g.inner}px grid`).toBe(Math.min(fits, g.tiles));
			expect(g.overflow, 'grid overflows its box').toBeLessThanOrEqual(0);
		}
	});
}
