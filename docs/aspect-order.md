# Aspect sort order

`ASPECT_ORDER` in `crates/dumpstar-db/src/search.rs` is the canonical aspect
order behind the collection's `sort=aspects`. It is a constant: the sort never
queries the catalog for it. This page records how it was derived.

## Derivation (run once, 2026-10-16)

Walk a set's cards by collector number, leaving out Leaders and Bases (they are
numbered apart and are not evidence), and read off the aspects. Source: the
verbatim official-API capture of Spark of Rebellion in
`fixtures/swu/official-sor-pages/` (991 printings; no production catalog copy
was reachable from the aeon's host, so this is the one set checked on real
data). Standard-variant cards, by number:

| numbers | aspects |
| --- | --- |
| 31-43 | Vigilance + Villainy |
| 44-55 | Vigilance + Heroism |
| 56-78 | Vigilance |
| 79-92 | Command + Villainy |
| 93-104 | Command + Heroism |
| 105-127 | Command |
| 128-139 | Aggression + Villainy |
| 140-152 | Aggression + Heroism |
| 153-175 | Aggression |
| 176-187 | Cunning + Villainy |
| 188-200 | Cunning + Heroism |
| 201-224 | Cunning |
| 225-235 | Villainy |
| 236-246 | Heroism |
| 247-252 | no aspect |

Units, events and upgrades interleave inside a run; cost is not the numbering
key (it is not ordered inside a run), so it is the tiebreak.

Result: **Vigilance, Command, Aggression, Cunning, Villainy, Heroism**.

- Per primary aspect, the dual-aspect cards come first (Villainy before
  Heroism), then the single-aspect ones. So `[Vigilance, Villainy]` and
  `[Vigilance, Heroism]` sort before `[Vigilance]`, not after it.
- Villainy alone, then Heroism alone, follow Cunning; unaspected cards are last.
- The sort compares the list as printed (the order stored and displayed), element
  by element, ranking each element by `ASPECT_ORDER`; it is not canonicalised
  into a set. `[Cunning, Vigilance, Villainy]` therefore sorts among the Cunning
  cards, and `[Villainy, Vigilance]` among the Villainy ones.

## Sort key

The rank in `ASPECT_ORDER` of each aspect in printed order, as two digits, closed by `99`
(`[Vigilance, Heroism]` is `010699`, `[Vigilance]` is `0199`); no aspects is
NULL and sorts last in both directions. Then cost, then name, then number.

## Guard

`aspect_order_constant_matches_the_set_numbering` re-derives the order from a
fixture set (the first aspect to stand alone on a card, walking by number) and
asserts it equals the constant. A real set that breaks the pattern should be
added to that fixture so the failure is a test, not a silent reorder.
