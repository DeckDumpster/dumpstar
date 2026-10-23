/**
 * The Decks page lists deck groups, each expandable to its decks, and a group
 * page acts on the whole unit. Both must hold on a phone: no sideways scroll,
 * nothing wider than the viewport. The shots are attached to the report as
 * review evidence (there are no pixel baselines in this tier).
 */

import { test, expect, type Page } from '@playwright/test';
import { stabilize, settle } from './stabilize';

const VIEWPORTS = [
	{ name: 'phone', width: 390, height: 844 },
	{ name: 'desktop', width: 1280, height: 800 }
];

async function overflowsSideways(page: Page): Promise<boolean> {
	return page.evaluate(
		() => document.documentElement.scrollWidth > document.documentElement.clientWidth
	);
}

for (const vp of VIEWPORTS) {
	test(`/decks lists groups with their decks, then the ungrouped, at ${vp.name}`, async ({
		page
	}, info) => {
		await page.setViewportSize({ width: vp.width, height: vp.height });
		await stabilize(page);
		await page.goto('/decks', { waitUntil: 'domcontentloaded' });
		await settle(page, '.group');

		const group = page.locator('details.group', { hasText: 'Weekend lists' });
		await expect(group).toBeVisible();
		await group.locator('summary').click();
		await expect(group.locator('.tile')).toHaveCount(2);
		// Ungrouped decks sit below the groups.
		const below = page.locator('h2', { hasText: 'Ungrouped decks' });
		await expect(below).toBeVisible();
		const [g, u] = await Promise.all([group.boundingBox(), below.boundingBox()]);
		expect(u!.y).toBeGreaterThan(g!.y + g!.height - 1);

		expect(await overflowsSideways(page), 'the Decks page scrolls sideways').toBe(false);
		await info.attach(`decks-${vp.name}`, {
			body: await page.screenshot({ fullPage: true }),
			contentType: 'image/png'
		});
	});

	test(`a group page shows totals, its decks and the whole-unit actions at ${vp.name}`, async ({
		page
	}, info) => {
		await page.setViewportSize({ width: vp.width, height: vp.height });
		await stabilize(page);
		await page.goto('/deck-groups/1', { waitUntil: 'domcontentloaded' });
		await settle(page, '.tile');

		await expect(page.locator('h1')).toHaveText('Weekend lists');
		await expect(page.locator('.sub')).toContainText('2 decks');
		await expect(page.locator('.grid .tile')).toHaveCount(2);
		for (const name of ['Rename or describe', 'Release every deck', 'Delete group']) {
			await expect(page.getByRole('button', { name })).toBeVisible();
		}

		// Delete asks which way: keep the decks, or delete them with the group.
		await page.getByRole('button', { name: 'Delete group' }).click();
		await expect(page.getByRole('button', { name: 'Keep the decks, ungrouped' })).toBeVisible();
		await expect(page.getByRole('button', { name: 'Delete the decks too' })).toBeVisible();

		expect(await overflowsSideways(page), 'the group page scrolls sideways').toBe(false);
		await info.attach(`deck-group-${vp.name}`, {
			body: await page.screenshot({ fullPage: true }),
			contentType: 'image/png'
		});
	});
}
