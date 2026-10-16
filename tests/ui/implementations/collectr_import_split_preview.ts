/**
 * Hand-written implementation for collectr_import_split_preview.
 *
 * Pastes a mixed Collectr export and asserts the preview renders three
 * separate panes — Single cards, Sealed products, Skipped — proving the
 * garden wall holds in the UI. Assertions target the preview's own DOM
 * (the `.pane-head` sections and backend-generated text like the resolved
 * set code and the skip reason) rather than text that also appears in the
 * Collectr hint paragraph or the pasted CSV.
 */
import type { ReplayHarness } from '../replay';

// 7 data rows: Lorcana (skipped); Spark of Rebellion Luke Skywalker - Faithful
// Friend #5 foil (matched); Millennium Falcon - Piece of Junk #193 qty 2
// (matched, 2 copies); Nonexistent Card #999 (unmatched card); Two-Player
// Starter (matched sealed); Shadows of the Galaxy pack qty 6 (matched sealed);
// Nonexistent Box (unmatched sealed).
const CSV = `Portfolio Name,Category,Set,Product Name,Card Number,Rarity,Variance,Grade,Card Condition,Average Cost Paid,Quantity,Market Price (As of 2026-07-19),Price Override,Watchlist,Date Added,Notes
Main,Lorcana,Disney Lorcana Promo Cards,Elsa - The Fifth Spirit,6,Promo,Foil,Ungraded,Near Mint,0,1,7.12,0,false,2026-01-24,
Main,Star Wars Unlimited,Spark of Rebellion,Luke Skywalker - Faithful Friend,5,Special,Foil,Ungraded,Lightly Played,280,1,300,0,false,2024-01-10,
Main,Star Wars Unlimited,Spark of Rebellion,Millennium Falcon - Piece of Junk,193,Rare,Normal,Ungraded,Near Mint,1.00,2,2,0,false,2024-01-10,
Main,Star Wars Unlimited,Spark of Rebellion,Nonexistent Card,999,Common,Normal,Ungraded,Near Mint,0,1,0,0,false,2024-01-10,
Sealed Star Wars Unlimited,Star Wars Unlimited,Spark of Rebellion,Spark of Rebellion Two-Player Starter,,,Normal,Ungraded,Near Mint,49.99,1,60,0,false,2023-09-22,
Sealed Star Wars Unlimited,Star Wars Unlimited,Shadows of the Galaxy,Shadows of the Galaxy Booster Pack,,,Normal,Ungraded,Near Mint,4.49,6,5,0,false,2024-11-08,
Sealed Star Wars Unlimited,Star Wars Unlimited,Spark of Rebellion,Nonexistent Box,,,Normal,Ungraded,Near Mint,10,1,10,0,false,2024-01-01,`;

export async function steps(h: ReplayHarness) {
  await h.navigate('/ingest/csv');
  await h.wait_for_text('Import CSV', 8000);
  await h.select_by_label('select', 'Collectr (cards + sealed)');
  await h.fill_by_selector('textarea', CSV);
  await h.click_by_text('Preview');

  // The preview renders three distinct panes (Single cards, Sealed products,
  // Skipped) — `.pane-head` exists only in the preview, not the hint.
  await h.wait_for_visible('.pane-head', 8000);
  await h.assert_element_count('.pane-head', 3);

  // A matched card resolved to its catalog set code (`SOR`) — this text is
  // produced by the backend, so it only appears in the Single cards pane.
  await h.assert_text_present('SOR');
  // Each split pane surfaced its own unmatched sub-table.
  await h.assert_text_present('Unmatched cards');
  await h.assert_text_present('Unmatched sealed');
  // The other-game row is reported skipped with a backend-generated reason.
  await h.assert_text_present("not a Star Wars: Unlimited category");
  await h.screenshot('final_state');
}
