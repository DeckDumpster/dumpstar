/**
 * Guards sp-yc7gp1: a set's sort buttons re-order the visible list and never
 * throw. A slot is one collector number, so a card's Standard and Hyperspace
 * printings are two slots with one card_id; keying the tile lists by card_id
 * made Svelte throw each_key_duplicate as soon as a sort put them on one page
 * (Price), after which the page stopped responding.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  const errors: string[] = [];
  h.page.on('pageerror', (e) => errors.push(String(e)));

  const firstNumber = () =>
    h.page.evaluate(
      () => document.querySelector('[data-testid="portrait-grid"] .card-tile .num')?.textContent ?? '',
    );
  // After a press: wait for the first tile to differ from `before`. An empty
  // grid is the page between responses, not a re-ordering: counting it as a
  // change returned '' on a slow container and failed the next comparison.
  const press = async (label: string, before: string) => {
    await h.page.locator('button.sortbtn', { hasText: label }).first().click();
    await h.page.waitForFunction(
      (b) => {
        const n =
          document.querySelector('[data-testid="portrait-grid"] .card-tile .num')?.textContent ?? '';
        return n !== '' && n !== b;
      },
      before,
      { timeout: 15000 },
    ).catch(() => {
      throw new Error(`pressing ${label} left the first tile at ${before}; errors: ${errors.join(' | ')}`);
    });
    return firstNumber();
  };

  await h.navigate('/browse/SOR?cols=5&turn=0');
  await h.wait_for_visible('[data-testid="portrait-grid"] .slot', 6000);

  const asc = await firstNumber();
  const desc = await press('#', asc);
  const asc2 = await press('#', desc);
  if (asc2 !== asc) throw new Error(`# twice should return to ${asc}, got ${asc2}`);

  const priceDesc = await press('Price', asc2);
  const priceAsc = await press('Price', priceDesc);
  await press('Price', priceAsc);

  if (errors.length) throw new Error(`uncaught exceptions: ${errors.join(' | ')}`);
  await h.screenshot('final_state');
}
