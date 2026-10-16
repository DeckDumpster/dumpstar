// covers: UC-card-15
/**
 * Guards sp-iszds1: the aspect facet searches by aspect across the catalog.
 */
import type { ReplayHarness } from '../replay';

const ASPECT = 'a.facet[title="Find all Aggression cards"]';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible(ASPECT, 6000);
  await h.click_by_selector(ASPECT);
  await h.page.waitForURL(/\/collection\?/, { timeout: 6000 });
  const url = h.page.url();
  if (!url.includes('q=aspect')) throw new Error(`expected an aspect: DSL query, got: ${url}`);
  if (!url.includes('all=1')) throw new Error(`expected catalog-wide all=1, got: ${url}`);
  await h.wait_for_visible('.cardtile[title^="Wampa"]', 6000);
  await h.assert_hidden('.cardtile[title^="Battlefield Marine"]');
  await h.screenshot('final_state');
}
