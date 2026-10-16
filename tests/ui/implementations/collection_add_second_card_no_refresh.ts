/**
 * Hand-written implementation for collection_add_second_card_no_refresh.
 *
 * Add a copy from one card's modal, close it, open a *different* card's modal,
 * and add a copy there too — the "+ Add" workflow must stay functional across
 * consecutive modals without a full page refresh. Vanquish (SOR #P01, 3 copies)
 * then Millennium Falcon (Showcase) (SHD #500, 3 copies) are both owned in the fixture.
 *
 * Note: the original hint predates the modal UI (it describes navigating back
 * to /collection between cards). Closing the modal is the modern equivalent;
 * the second add proves the workflow survives the first mutation + close.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('.cardtile', 4000);

  // First card: Vanquish (3 → 4 copies).
  await h.click_by_selector('.cardtile[title^="Vanquish"]');
  await h.wait_for_visible('[role="dialog"]', 4000);
  await h.assert_text_present('Your copies (3)');
  await h.click_by_selector('[role="dialog"] button[aria-label^="Add one"]');
  await h.wait_for_text('Your copies (4)', 4000);
  await h.click_by_selector('[role="dialog"] .x');
  await h.wait_for_hidden('[role="dialog"]', 4000);

  // Second card: Millennium Falcon (Showcase) (3 → 4 copies) — the workflow still works.
  await h.click_by_selector('.cardtile[title*="(Showcase)"]');
  await h.wait_for_visible('[role="dialog"]', 4000);
  await h.assert_text_present('Your copies (3)');
  await h.click_by_selector('[role="dialog"] button[aria-label^="Add one"]');
  await h.wait_for_text('Your copies (4)', 4000);
  await h.assert_text_present('Your copies (4)');
  await h.screenshot('final_state');
}
