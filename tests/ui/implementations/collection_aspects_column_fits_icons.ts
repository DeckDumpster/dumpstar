/**
 * Hand-written implementation for collection_aspects_column_fits_icons.
 *
 * The Aspects column is as wide as its widest content: two aspect icons plus
 * cell padding (+1px). A two-aspect card's icons must not be clipped.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('[data-testid="search-input"]', 6000);
  await h.click_by_test_id('view-table');
  await h.wait_for_visible('table.dd tbody tr', 6000);

  const m = await h.page.evaluate(() => {
    const ths = Array.from(document.querySelectorAll('table.dd thead th'));
    const idx = ths.findIndex((t) => t.textContent?.trim() === 'Aspects');
    if (idx < 0) return null;
    const th = ths[idx] as HTMLElement;
    const pad = parseFloat(getComputedStyle(th).paddingLeft) + parseFloat(getComputedStyle(th).paddingRight);
    let two: { icons: number; clipped: boolean } | null = null;
    for (const tr of Array.from(document.querySelectorAll('table.dd tbody tr'))) {
      const td = tr.children[idx] as HTMLElement | undefined;
      const imgs = td ? Array.from(td.querySelectorAll('img.aspect-icon')) : [];
      if (td && imgs.length === 2) {
        const c = td.getBoundingClientRect();
        const last = imgs[1].getBoundingClientRect();
        two = { icons: imgs.length, clipped: last.right > c.right - pad / 2 + 0.5 || last.left < c.left };
        break;
      }
    }
    const icon = document.querySelector('table.dd tbody img.aspect-icon')?.getBoundingClientRect().width ?? 0;
    return { width: th.getBoundingClientRect().width, pad, icon, two };
  });
  if (!m) throw new Error('no Aspects column header');
  if (m.width > 2 * m.icon + m.pad + 1) {
    throw new Error(`Aspects column ${m.width}px exceeds two icons (${m.icon}px) + padding (${m.pad}px) + 1px`);
  }
  if (!m.two) throw new Error('no two-aspect card rendered in the table');
  if (m.two.clipped) throw new Error('two-aspect icons are clipped by the Aspects column');
  await h.screenshot('final_state');
}
