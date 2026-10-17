// covers: UC-collection-04
/**
 * Hand-written implementation for collection_aspects_column_fits_icons.
 *
 * The Aspects column is as wide as the card with the most aspects in the
 * result needs: every icon of that card sits on one line (same offsetTop) and
 * none is clipped, at desktop and mobile widths.
 */
import type { ReplayHarness } from '../replay';

async function measure(h: ReplayHarness) {
  return h.page.evaluate(() => {
    const ths = Array.from(document.querySelectorAll('table.dd thead th'));
    const idx = ths.findIndex((t) => t.textContent?.trim() === 'Aspects');
    if (idx < 0) return null;
    let widest: { icons: number; sameLine: boolean; clipped: boolean } | null = null;
    for (const tr of Array.from(document.querySelectorAll('table.dd tbody tr'))) {
      const td = tr.children[idx] as HTMLElement | undefined;
      const imgs = td ? (Array.from(td.querySelectorAll('img.aspect-icon')) as HTMLElement[]) : [];
      if (!td || imgs.length === 0 || (widest && imgs.length <= widest.icons)) continue;
      const c = td.getBoundingClientRect();
      const last = imgs[imgs.length - 1].getBoundingClientRect();
      widest = {
        icons: imgs.length,
        sameLine: imgs.every((i) => i.offsetTop === imgs[0].offsetTop),
        clipped: last.right > c.right + 0.5 || imgs[0].getBoundingClientRect().left < c.left,
      };
    }
    return widest;
  });
}

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('[data-testid="search-input"]', 20000);
  await h.click_by_test_id('view-table');
  await h.wait_for_visible('table.dd tbody tr', 20000);

  for (const width of [1280, 390]) {
    await h.page.setViewportSize({ width, height: 900 });
    const m = await measure(h);
    if (!m) throw new Error(`no aspect icons in the table at ${width}px`);
    if (m.icons < 3) throw new Error(`expected a three-aspect card in the table, widest has ${m.icons}`);
    if (!m.sameLine) throw new Error(`${m.icons} aspect icons wrap onto several lines at ${width}px`);
    if (m.clipped) throw new Error(`aspect icons are clipped by the Aspects column at ${width}px`);
  }
  await h.screenshot('final_state');
}
