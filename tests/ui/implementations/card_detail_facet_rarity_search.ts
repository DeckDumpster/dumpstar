/**
 * Hand-written implementation for card_detail_facet_rarity_search.
 *
 * Guards dumpstar-xj6: the rarity link now emits rarity:"<R>"&all=1
 * instead of a bare name search. Luke Skywalker (SOR/005) is Special, as is
 * SOR/262 (Hyperspace) — so the search returns both, the Hyperspace card
 * among them.
 */
import type { ReplayHarness } from '../replay';

const RARITY = 'a.facet[title="Find all Special cards"]';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible(RARITY, 6000);
  await h.click_by_selector(RARITY);
  await h.page.waitForURL(/\/collection\?/, { timeout: 6000 });
  const url = h.page.url();
  if (!url.includes('q=rarity')) throw new Error(`expected a rarity: DSL query, got: ${url}`);
  if (!url.includes('all=1')) throw new Error(`expected catalog-wide all=1, got: ${url}`);
  await h.wait_for_visible('.cardtile[title*="(Hyperspace)"]', 6000);
  await h.screenshot('final_state');
}
