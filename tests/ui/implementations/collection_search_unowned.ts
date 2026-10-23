// covers: UC-collection-30
/**
 * Hand-written implementation for collection_search_unowned.
 *
 * With "All cards" on, a t:unit query surfaces unowned printings as dimmed
 * "missing" grid tiles alongside owned ones.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.navigate('/collection');
  await h.wait_for_visible('[data-testid="search-input"]');
  // Grid view renders the dimmed .missing tiles for unowned printings.
  await h.click_by_test_id('view-grid');
  // is:unowned in the query makes the search catalog-wide.
  await h.fill_by_selector('[data-testid="search-input"]', 'is:unowned t:unit');
  // At least one unowned (missing) tile appears.
  await h.wait_for_visible('.cardtile.missing');
  await h.assert_visible('.cardtile.missing');
  await h.screenshot('final_state');
}
