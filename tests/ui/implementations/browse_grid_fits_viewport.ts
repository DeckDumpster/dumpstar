/**
 * Guards sp-mxm9dw: the set browse grid never overflows the viewport, and the
 * cards in one row share a height, at every cards-per-row setting.
 */
import type { ReplayHarness } from '../replay';

const WIDTHS = [1280, 390];
const COLS = [1, 3, 8];
const SETS = ['SOR', 'HMW'];

export async function steps(h: ReplayHarness) {
  for (const width of WIDTHS) {
    await h.page.setViewportSize({ width, height: 900 });
    for (const set of SETS) for (const cols of COLS) {
      await h.navigate(`/browse/${set}?cols=${cols}&view=card`);
      await h.wait_for_visible('[data-testid="landscape-grid"] .slot', 6000);
      await h.wait_for_visible('[data-testid="portrait-grid"] .slot', 6000);

      const report = await h.page.evaluate(() => {
        const out: string[] = [];
        const doc = document.documentElement;
        if (doc.scrollWidth > doc.clientWidth) {
          out.push(`page scrollWidth ${doc.scrollWidth} > clientWidth ${doc.clientWidth}`);
        }
        const controls = document.querySelector<HTMLElement>('.controls');
        const edge = controls ? controls.getBoundingClientRect().right : doc.clientWidth;
        for (const id of ['landscape-grid', 'portrait-grid']) {
          const grid = document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
          if (!grid) continue;
          if (grid.scrollWidth > grid.clientWidth) {
            out.push(`${id} scrollWidth ${grid.scrollWidth} > clientWidth ${grid.clientWidth}`);
          }
          const gridRight = grid.getBoundingClientRect().right;
          if (Math.abs(gridRight - edge) > 1) out.push(`${id} right ${gridRight} != controls right ${edge}`);
          const caps = new Map<number, number[]>();
          for (const m of grid.querySelectorAll<HTMLElement>('.slotwrap .meta')) {
            const top = Math.round(m.closest('.slotwrap')!.getBoundingClientRect().top);
            caps.set(top, [...(caps.get(top) ?? []), m.getBoundingClientRect().top]);
          }
          for (const [top, ys] of caps) {
            if (Math.max(...ys) - Math.min(...ys) > 1) out.push(`${id} row at ${top} has unequal caption offsets ${ys.join(',')}`);
          }
          const rows = new Map<number, number[]>();
          for (const img of grid.querySelectorAll<HTMLElement>('.slot img, .slot .noart')) {
            const r = img.getBoundingClientRect();
            if (r.right > gridRight + 1) out.push(`${id} card exceeds grid: ${r.right} > ${gridRight}`);
            const top = Math.round(img.closest('.slotwrap')!.getBoundingClientRect().top);
            rows.set(top, [...(rows.get(top) ?? []), r.height]);
          }
          for (const [top, hs] of rows) {
            if (Math.max(...hs) - Math.min(...hs) > 1) {
              out.push(`${id} row at ${top} has unequal heights ${hs.join(',')}`);
            }
          }
        }
        return out;
      });
      if (report.length) {
        throw new Error(`${set} width ${width}, cols ${cols}: ${report.join('; ')}`);
      }
    }
  }
  await h.page.setViewportSize({ width: 390, height: 900 });
  await h.navigate('/browse/HMW?cols=3&view=card');
  await h.screenshot('final_state');
}
