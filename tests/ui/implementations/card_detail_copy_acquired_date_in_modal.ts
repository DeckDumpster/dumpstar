// covers: UC-card-26
/**
 * Guards sp-kx1odm: the card modal lists each owned copy's acquisition date.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('.cardtile', 4000);
  await h.click_by_selector('.cardtile[title^="Death Star Stormtrooper"]');
  await h.wait_for_visible('[role="dialog"]', 4000);
  await h.assert_text_present('Your copies (1)');
  await h.assert_text_present('Acquired');
  await h.assert_visible('[role="dialog"] [data-testid="copy-acquired"]');
  await h.assert_text_present('2025-06-10');
  await h.screenshot('final_state');
}
