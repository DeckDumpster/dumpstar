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

test("only groups whose sets hold an owned card start expanded", async () => {
  const { startsCollapsed } = await import("$lib/setGroups");
  /** @param {string} code @param {string} category @param {number} n */
  const owned = (code, category, n) => ({ ...s(code, category), owned_cards: n });
  const groups = groupSets([
    owned("SOR", "Main", 0),
    owned("LOF", "Main", 5),
    owned("P1", "Promo", 0),
    owned("T1", "Starter", 0),
  ]);
  assert.deepEqual(
    groups.map((g) => [g.category, startsCollapsed(g)]),
    [["Main", false], ["Promo", true], ["Starter", true]]
  );
});
