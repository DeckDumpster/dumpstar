/**
 * Hand-written implementation for rarity_official_mark (sp-kvga20).
 * The character, colour and sort order are catalog data served by
 * /api/rarities; a Legendary card shows a vector mark, no text, in rgb(0, 200, 243).
 */
import type { ReplayHarness } from '../replay';

const MARK = 'span.rarity-mark[aria-label="Legendary"]';

async function assertMark(h: ReplayHarness) {
  await h.wait_for_visible(MARK, 6000);
  const got = await h.page.locator(MARK).first().evaluate((el) => ({
    text: el.textContent?.trim(),
    svg: el.querySelectorAll('svg path').length,
    color: getComputedStyle(el).color,
  }));
  if (got.text) throw new Error(`expected no text label, got "${got.text}"`);
  if (got.svg !== 1) throw new Error(`expected one svg mark, got ${got.svg}`);
  if (got.color !== 'rgb(0, 200, 243)') throw new Error(`expected rgb(0, 200, 243), got ${got.color}`);
}

export async function steps(h: ReplayHarness) {
  await assertMark(h);
  await h.navigate('/collection?q=rarity%3Alegendary&all=1');
  await assertMark(h);
  // Rarity sort follows the stored sort values: Special (1) is the lowest,
  // Legendary (5) the highest.
  await h.navigate('/collection?q=rarity%3Alegendary%20or%20rarity%3Aspecial&all=1');
  await h.click_by_text('Rarity');
  await h.page.waitForTimeout(500);
  const labels = await h.page
    .locator('span.rarity-mark')
    .evaluateAll((els) => els.map((e) => e.getAttribute('aria-label')));
  const first = labels[0];
  const last = labels[labels.length - 1];
  if (!(first && last && first !== last))
    throw new Error(`rarity sort did not order Special against Legendary: ${labels.join(',')}`);
  await h.screenshot('final_state');
}
