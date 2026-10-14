/**
 * Hand-written implementation for collection_search_keyword_filter.
 *
 * Types an `aspect:Aggression` query and asserts the server-side filter narrows
 * the table to Aggression cards (Wampa present, Command Battlefield Marine absent).
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.navigate('/collection');
  await h.wait_for_visible('[data-testid="search-input"]');
  // Table view renders card names as text, which we can assert on.
  await h.click_by_test_id('view-table');
  // Debounced, server-side query.
  await h.fill_by_selector('[data-testid="search-input"]', 'aspect:Aggression');
  await h.wait_for_text('Wampa');
  await h.assert_text_present('Wampa');
  // Battlefield Marine is a Command card — excluded by aspect:Aggression.
  await h.assert_text_absent('Battlefield Marine');
  await h.screenshot('final_state');
}
