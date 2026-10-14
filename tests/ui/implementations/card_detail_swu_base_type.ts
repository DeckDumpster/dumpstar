/**
 * Guards sp-iszds1: a base shows its type and HP and nothing it lacks.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('a.facet[title="Find all Base cards"]', 6000);
  await h.assert_text_present('HP');
  for (const gone of ['Power', 'Cost', 'Arena']) {
    await h.assert_text_absent(gone);
  }
  await h.assert_element_count('img.energy', 0);
  await h.screenshot('final_state');
}
