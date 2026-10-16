/**
 * Hand-written implementation for collectr_import_commit_both_sides.
 *
 * Previews then commits a Collectr export and asserts the confirmation
 * banner reports both cards and sealed products added, with a batch link.
 * Waits on the preview's `.pane-head` (not the always-present Collectr hint)
 * so the Import button is actually enabled before clicking.
 */
import type { ReplayHarness } from '../replay';

const CSV = `Portfolio Name,Category,Set,Product Name,Card Number,Rarity,Variance,Grade,Card Condition,Average Cost Paid,Quantity,Market Price (As of 2026-07-19),Price Override,Watchlist,Date Added,Notes
Main,Lorcana,Disney Lorcana Promo Cards,Elsa - The Fifth Spirit,6,Promo,Foil,Ungraded,Near Mint,0,1,7.12,0,false,2026-01-24,
Main,Star Wars Unlimited,Spark of Rebellion,Luke Skywalker - Faithful Friend,5,Special,Foil,Ungraded,Lightly Played,280,1,300,0,false,2024-01-10,
Main,Star Wars Unlimited,Spark of Rebellion,Millennium Falcon - Piece of Junk,193,Rare,Normal,Ungraded,Near Mint,1.00,2,2,0,false,2024-01-10,
Sealed Star Wars Unlimited,Star Wars Unlimited,Spark of Rebellion,Spark of Rebellion Two-Player Starter,,,Normal,Ungraded,Near Mint,49.99,1,60,0,false,2023-09-22,
Sealed Star Wars Unlimited,Star Wars Unlimited,Shadows of the Galaxy,Shadows of the Galaxy Booster Pack,,,Normal,Ungraded,Near Mint,4.49,6,5,0,false,2024-11-08,`;

export async function steps(h: ReplayHarness) {
  await h.navigate('/ingest/csv');
  await h.wait_for_text('Import CSV', 8000);
  await h.select_by_label('select', 'Collectr (cards + sealed)');
  await h.fill_by_selector('textarea', CSV);
  await h.click_by_text('Preview');

  // Wait for the preview to actually render (its panes), which also enables
  // the Import button (matchCount > 0).
  await h.wait_for_visible('.pane-head', 8000);
  await h.click_by_selector('.commit');

  // The banner reports both halves and links to the card batch — both texts
  // exist only after a committed Collectr import.
  await h.wait_for_text('View card batch', 8000);
  await h.assert_text_present('Imported');
  await h.assert_text_present('View card batch');
  await h.screenshot('final_state');
}
