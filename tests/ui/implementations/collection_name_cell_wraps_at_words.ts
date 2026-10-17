// covers: UC-collection-46
/**
 * Hand-written implementation for collection_name_cell_wraps_at_words.
 *
 * At 390px, for every rendered row: each word of the card name lies on one
 * line (a word broken mid-word has two client rects), and the name cell's
 * content is not clipped (scrollHeight <= clientHeight).
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('[data-testid="search-input"]', 6000);
  await h.click_by_test_id('view-table');
  await h.page.setViewportSize({ width: 390, height: 900 });
  await h.wait_for_visible('table.dd tbody tr', 6000);

  const bad = await h.page.evaluate(() => {
    const problems: string[] = [];
    for (const cell of Array.from(document.querySelectorAll('table.dd tbody td.namecol .namecell')) as HTMLElement[]) {
      const name = cell.querySelector('.cardname') as HTMLElement | null;
      const text = name?.firstChild;
      if (!name || !text || text.nodeType !== Node.TEXT_NODE) continue;
      const s = text.textContent ?? '';
      for (const m of s.matchAll(/\S+/g)) {
        const r = document.createRange();
        r.setStart(text, m.index!);
        r.setEnd(text, m.index! + m[0].length);
        if (r.getClientRects().length > 1) problems.push(`word "${m[0]}" of "${s}" is broken across lines`);
      }
      if (cell.scrollHeight > cell.clientHeight) problems.push(`name cell of "${s}" is clipped`);
      const body = cell.querySelector('.namebody') as HTMLElement | null;
      if (body && body.scrollHeight > body.clientHeight) problems.push(`name of "${s}" is clipped`);
    }
    return problems;
  });
  if (bad.length) throw new Error(bad.join('; '));
  await h.screenshot('final_state');
}
