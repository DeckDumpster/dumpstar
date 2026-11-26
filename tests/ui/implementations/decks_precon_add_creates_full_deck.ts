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
const ROW = `li:has-text("${DECK}")`;
const TAB = 'button.segment:has-text("Preconstructed")';
const ADD = `${ROW} button:has-text("Add to collection and create deck")`;

/** Wait for the new deck's page, check it is full, and return its path. */
async function fullDeck(h: ReplayHarness): Promise<string> {
  await h.page.waitForURL(/\/decks\/\d+$/);
  await h.wait_for_visible('li.line');
  await h.assert_visible(`h1:text-is("${DECK}")`);
  await h.assert_element_count('li.line', 25);
  const quantities = await h.page.locator('li.line .qty').allTextContents();
  const total = quantities.reduce((n, q) => n + parseInt(q, 10), 0);
  if (total !== 52) throw new Error(`the deck holds ${total} of 52 cards`);
  return new URL(h.page.url()).pathname;
}

export async function steps(h: ReplayHarness) {
  await h.navigate('/decks');
  // The plain name form is the tab the page opens on.
  await h.fill_by_placeholder('New deck name…', 'Rogue Squadron');
  await h.click_by_selector('button:text-is("Create")');
  await h.wait_for_visible('a.tile:has-text("Rogue Squadron")');

  await h.click_by_selector(TAB);
  await h.wait_for_text(DECK);

  // A failure that is not a repeat goes to the page's error line.
  const addRoute = '**/api/precons/*/add';
  await h.page.route(addRoute, (r) => r.fulfill({ status: 500, body: 'boom' }));
  await h.click_by_selector(ADD);
  await h.wait_for_visible('p.error:has-text("500")');
  await h.page.unroute(addRoute);

  await h.click_by_selector(ADD);
  const first = await fullDeck(h);
  await h.screenshot('final_state');

  // Adding it again is refused until confirmed, then makes a second deck.
  await h.navigate('/decks');
  await h.click_by_selector(TAB);
  await h.click_by_selector(ADD);
  await h.wait_for_visible(`${ROW} [role="alert"]:has-text("already added as deck")`);
  await h.click_by_selector(`${ROW} button:has-text("Add a second copy")`);
  const second = await fullDeck(h);
  if (second === first) throw new Error(`the repeat add landed on the first deck, ${first}`);
}
