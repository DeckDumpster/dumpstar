// covers: UC-browse-01
/**
 * Hand-written implementation for browse_back_preserves_page_param.
 *
 * Regression guard for dumpstar-k7v: reaching a deep binder page via the
 * pager (a client-side URL rewrite) then navigating to /card and back used to
 * drop the ?page= param and snap to page 1. The fix swapped the URL-sync from
 * $app/navigation's replaceState (which stashes the *stale* page.url as the
 * history-entry restore key) to goto(replaceState:true).
 *
 * Uses cols=1 so the 4-card SOR fixture paginates into 2 pages.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  // cols=1 -> 3 cards/page -> SOR's 4 cards span 2 pages.
  await h.navigate('/browse/SOR?cols=1');
  await h.wait_for_visible('.slot');

  // Page forward to page 2 via the pager (client-side URL rewrite).
  await h.click_by_selector('.pager-bottom button:has-text("Next")');
  await h.wait_for_text('Page 2 of 2');
  if (!h.page.url().includes('page=2')) {
    throw new Error(`expected page=2 in URL, got ${h.page.url()}`);
  }

  // Open the variant modal and follow "Full card details".
  await h.click_by_selector('.slot');
  await h.wait_for_visible('.modal');
  await h.click_by_selector('a.full');
  await h.page.waitForURL(/\/card\//, { timeout: 5000 });

  // Browser Back must restore page 2, not snap to page 1.
  await h.page.goBack({ waitUntil: 'networkidle' });
  await h.wait_for_text('Page 2 of 2');
  await h.assert_text_present('Page 2 of 2');
  if (!h.page.url().includes('page=2')) {
    throw new Error(`Back dropped the page param: ${h.page.url()}`);
  }
  await h.screenshot('final_state');
}
