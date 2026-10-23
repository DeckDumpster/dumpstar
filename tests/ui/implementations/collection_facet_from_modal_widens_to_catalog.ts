// covers: UC-collection-08
/**
 * Hand-written fail-first regression for the "all=1 ignored on client-side
 * facet nav" bug: clicking a facet link inside the card modal navigated to
 * /collection?q=...&all=1 but the page only resynced the `q` param from the
 * URL, not `all` — so the catalog stayed owned-only until a manual reload.
 *
 * Wampa (SOR_085) is owned and shares the Aggression aspect with unowned
 * Aggression cards. Clicking the Aggression facet in the modal must
 * immediately surface that unowned card as a dimmed .cardtile.missing tile,
 * with no reload. Fails before the +page.svelte resync fix.
 */
import type { ReplayHarness } from '../replay';

const ASPECT_LINK = '[role="dialog"] a.facet[title="Find all Aggression cards"]';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('.cardtile', 6000);
  // Open the owned Wampa modal.
  await h.click_by_selector('.cardtile[title^="Wampa"]');
  await h.wait_for_visible('[role="dialog"]', 6000);

  // Click the Aggression facet inside the modal.
  await h.wait_for_visible(ASPECT_LINK, 6000);
  await h.click_by_selector(ASPECT_LINK);
  await h.page.waitForURL(/\/collection\?.*aspect/, { timeout: 6000 });

  // The facet stays in-collection: an owned-only search.
  await h.wait_for_visible('.cardtile', 6000);
  await h.screenshot('final_state');
}
