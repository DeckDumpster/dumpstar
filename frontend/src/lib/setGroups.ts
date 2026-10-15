import type { SetSummary } from "$lib/types/SetSummary";

/** A product category and its sets, in the order the API sent them. */
export type SetGroup = { category: string; slug: string; sets: SetSummary[] };

/** Heading for a set the catalog's declared category mapping lacks. */
export const UNCATEGORISED = "Uncategorised";

/**
 * Group sets by SWU product category. The API already orders the list —
 * main expansions first, each category newest first — so groups keep first-seen
 * order and nothing is re-sorted here.
 */
export function groupSets(sets: SetSummary[]): SetGroup[] {
  const groups: SetGroup[] = [];
  for (const set of sets) {
    const category = set.category ?? UNCATEGORISED;
    let g = groups.find((x) => x.category === category);
    if (!g) {
      g = {
        category,
        slug: category.toLowerCase().replace(/[^a-z0-9]+/g, "-"),
        sets: [],
      };
      groups.push(g);
    }
    g.sets.push(set);
  }
  return groups;
}
