import test from "node:test";
import assert from "node:assert/strict";
import { groupSets, UNCATEGORISED } from "$lib/setGroups";

/**
 * @param {string} set_code
 * @param {string | null} category
 * @returns {import("$lib/types/SetSummary").SetSummary}
 */
const s = (set_code, category) => ({
  set_code,
  category,
  name: set_code,
  release_date: null,
  total_cards: 0,
  owned_cards: 0,
  total_printings: 0,
  owned_printings: 0,
  logo_url: null,
  art_url: null,
});

test("groupSets keeps the API order: categories as first seen, sets within them", () => {
  const groups = groupSets([
    s("SHD", "Main Expansions"),
    s("SOR", "Main Expansions"),
    s("JTLP", "Weekly Play Promos"),
    s("IBH", "Intro & Starter Products"),
  ]);
  assert.deepEqual(
    groups.map((g) => g.category),
    ["Main Expansions", "Weekly Play Promos", "Intro & Starter Products"],
  );
  assert.deepEqual(
    groups[0].sets.map((x) => x.set_code),
    ["SHD", "SOR"],
  );
  assert.equal(groups[2].slug, "intro-starter-products");
});

test("groupSets names an unmapped set rather than hiding it in another category", () => {
  const groups = groupSets([s("SOR", "Main Expansions"), s("ZZZ", null)]);
  assert.equal(groups[1].category, UNCATEGORISED);
});
