// covers: UC-card-27
/**
 * Guards sp-kx1odm: the full card page lists each owned copy's acquisition date.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('[data-testid="copy-acquired"]', 6000);
  await h.assert_text_present('Your copies (1)');
  await h.assert_text_present('Acquired');
  await h.assert_visible('[data-testid="copy-acquired"]');
  await h.assert_text_present('2025-06-10');
  await h.screenshot('final_state');
}
