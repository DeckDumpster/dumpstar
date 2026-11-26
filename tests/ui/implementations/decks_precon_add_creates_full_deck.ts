// covers: UC-decks-10
/**
 * Hand-written implementation for decks_precon_add_creates_full_deck.
 *
 * The deck page lists only the copies a deck really holds, so "every line is
 * filled" is the line quantities adding up to the decklist's 52 cards over its
 * 25 lines; a missing or short line makes either number smaller.
 */
import type { ReplayHarness } from '../replay';

const DECK = 'Spotlight Deck: Boba Fett';

export async function steps(h: ReplayHarness) {
  await h.navigate('/decks');
  await h.click_by_selector('button.segment:has-text("Preconstructed")');
  await h.wait_for_text(DECK);
  await h.click_by_selector(
    `li:has-text("${DECK}") button:has-text("Add to collection and create deck")`,
  );
  await h.page.waitForURL(/\/decks\/\d+$/);
  await h.wait_for_visible('li.line');
  await h.assert_visible(`h1:text-is("${DECK}")`);
  await h.assert_element_count('li.line', 25);
  const quantities = await h.page.locator('li.line .qty').allTextContents();
  const total = quantities.reduce((n, q) => n + parseInt(q, 10), 0);
  if (total !== 52) throw new Error(`the deck holds ${total} of 52 cards`);
  await h.screenshot('final_state');
}
