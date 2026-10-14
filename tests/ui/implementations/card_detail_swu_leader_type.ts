/**
 * Guards sp-iszds1: a leader shows its type, cost, power and HP.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('a.facet[title="Find all Leader cards"]', 6000);
  await h.assert_text_present('Power');
  await h.assert_text_present('HP');
  await h.assert_text_present('Cost');
  await h.assert_element_count('img.energy', 0);
  await h.screenshot('final_state');
}
