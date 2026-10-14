# Intents UI Test Fixture

Deterministic Star Wars: Unlimited fixture for Dumpstar's intents UI-testing
harness. The harness snapshots/restores these two files before each test (see
`conftest.ts`); the running server opens them as its shared catalog and user
collection.

> **Scenario drift.** The intents, hints and visual routes name this
> fixture's SWU ids (`SOR_005`, `SOR-521:Normal`, `/card/SOR/005`), but the
> counts and stories some of them describe (batch sizes, deck contents,
> copy numbers) were written against an earlier seeding and have not been
> re-verified against this one. The visual baselines are stale until
> `bash tests/visual/run.sh --update` runs on a test VM.

## Regenerating

```bash
cargo run -p dumpstar-cli -- seed-fixture
# or, into an explicit directory:
cargo run -p dumpstar-cli -- seed-fixture --out tests/ui/fixtures
```

The command does a clean rebuild: it deletes any existing `shared.sqlite` /
`collection.sqlite`, recreates them through `dumpstar_db::open_shared` /
`connect_user` (so the SWU catalog schema, dumpstar-db's overlay and the shipped
seeds are applied), then seeds deterministic rows. Catalog rows are inserted
directly in `crates/dumpstar-cli/src/fixture.rs`; user data goes through the
`dumpstar-db` repository functions so app-layer validation runs.

**Both files are byte-stable.** Two regenerations of an unchanged seeder
produce two identical pairs (compare checksums), so `git status` after a
regeneration is silent unless something really moved.
`fixture::tests::two_seed_runs_are_byte_identical` is the gate. Every catalog
row, price and date in `shared.sqlite` is a fixed constant; `seed-fixture` pins
the clock for the build at 2025-06-15T09:00Z and advances it one minute per
read, so repository-stamped columns (`created_at`, `added_at`, ...) are
distinct and stable. Nothing is dated after the visual suite's frozen browser
clock (2026-01-15); `no_fixture_row_is_dated_after_the_frozen_browser_clock`
holds that.

Card names and set names/release dates are real; stats and rules text are
paraphrases written for a fixture, and the three `card_legality` rows are
invented to exercise the legality checker.

## Catalog (`shared.sqlite`)

3 sets, 26 cards, 86 printings (43 collector numbers x Normal/Foil), 111 price
rows, 4 builtin formats, 4 sealed products.

| Set | Name | Released |
| --- | --- | --- |
| `SOR` | Spark of Rebellion | 2024-03-08 |
| `SHD` | Shadows of the Galaxy | 2024-07-12 |
| `JTL` | Jump to Lightspeed | 2025-03-14 |

**Ids.** A card id is `<SET>_<number>` of its Standard printing (`SOR_005`). A
printing id is `<SET>-<number>:<Finish>` (`SOR-262:Foil`), the shape of
production's `<cardUid>:<Finish>`; the uid half (`SOR-262`) is the `official`
crosswalk id. Variants have their own collector numbers: Standard (`005`),
Hyperspace (`262`..`269`), Showcase (`515`, `520`, `521`) and Promo (`P01`..`P05`).

**Cards by type** (all ids are `card_id`):

| Type | Cards |
| --- | --- |
| Leader | `SOR_005` Luke Skywalker, Faithful Friend; `SOR_010` Darth Vader, Dark Lord of the Sith; `SHD_001` Cad Bane, Now It's Personal; `JTL_001` Wedge Antilles, Leader of Red Squadron |
| Base | `SOR_020` Echo Base; `SOR_024` Dagobah Swamp; `SHD_020` Level 1313 |
| Unit | `SOR_193` Millennium Falcon, Piece of Junk; `SOR_070` Battlefield Marine; `SOR_085` Wampa; `SOR_100` Death Star Stormtrooper; `SOR_120` Cartel Spacer; `SOR_130` TIE/ln Fighter; `SOR_140` Snowtrooper Lieutenant; `SOR_150` Fleet Lieutenant; `SOR_160` Vigilant Honor Guards; `SHD_100` Bossk, Deadly Stalker; `JTL_050` X-Wing |
| Event | `SOR_170` Vanquish; `SOR_171` Overwhelming Barrage; `SOR_172` Power of the Dark Side; `SOR_173` Waylay; `SOR_174` Repair |
| Upgrade | `SOR_180` Entrenched; `SOR_181` Fallen Lightsaber; `SOR_182` Jedi Lightsaber |

Rarities present: Common, Uncommon, Rare, Legendary, Special. Every card has a
`card_search` full-text row, written the way the catalog apply job writes it.

**Prices** (`prices`, source `tcgcsv`, one row per printing and day). The
newest quotes are dated `2025-06-15`; every third priced printing also has
`2025-06-14` (5% lower) and every seventh `2025-06-13` (10% lower), so some
prices have moved and some have not. Deliberately unpriced: the Foil of
`SOR-100`, `SOR-520`, `SOR-521`, `SHD-515` and `SOR-P02`; the Normal of
`SOR-P03`; and both finishes of `SOR-268`, `SOR-269` and `SOR-P05`. Chase
prices to know: `SOR-521:Normal` (Showcase Falcon) 140.00, `SOR-520:Normal`
(Showcase Luke) 85.00, `SHD-515:Normal` (Showcase Cad Bane) 45.00.

**Formats and legality.** `eternal`, `limited`, `premier`, `twin_suns` come from the
catalog crate's own `formats.json`. Invented legality rows: `SOR_193` is
`banned` and `SOR_172` `restricted` in `premier`; `SOR_170` is `not_legal` in
`twin_suns`.

**Sealed products** (`sealed_products`, `sealed_prices`):

| Product | Name | Quote | Held |
| --- | --- | --- | --- |
| 900001 | Spark of Rebellion Booster Box | none | no |
| 900002 | Shadows of the Galaxy Booster Box | market 96.40 (mid 94.75), also 2025-06-14: 94.80 | 1 |
| 900003 | Spark of Rebellion Two-Player Starter | none | no |
| 900004 | Jump to Lightspeed Booster Pack | mid 5.24, `market_price` NULL | 6 |

The two held lots are quoted the two ways on purpose: one by `market_price`,
one by `mid_price` alone (the `COALESCE` fallback).

## Collection (`collection.sqlite`)

31 `collection` rows (26 seeded copies, including the off-catalog one, plus 5 the two orders create), acquired `2025-06-10`. Stable names:

| Kind | Name | Notes |
| --- | --- | --- |
| Binder | `Trade Binder` | trade; CSV batch `CSV export 2025-06` |
| Binder | `Master Set: Spark of Rebellion` | set; batch `Spark of Rebellion binder page-through` |
| Binder | `Hyperspace Vault` | showcase; batch `Hyperspace entry`; holds `SOR-520:Normal` as a PSA 9 |
| Deck | `Luke Vigilance Midrange` | built, `premier`; legal: leader `SOR_005`, base `SOR_024`, 50 main |
| Deck | `Vader Aggro` | ready, `premier`; leader `SOR_010`, base `SOR_020`, partial |
| Deck | `Bounty Hunters` | idea, `premier`; `SHD_001` + 3 `SHD_100` |
| Order | `TCG-100001` | received; `SOR-070:Normal` x3, `SOR-170:Foil` |
| Order | `EBAY-55012` | shipped, still `ordered`; `SOR-520:Normal` |
| Sealed | `900002` x1 at 89.99; `900004` x6 at 4.49 | |
| Wishlist | `SHD_001` (`SHD-515:Normal`), `JTL_001` (any), `SOR_005` (`SOR-520:Normal`) | |

Other stories: one `listed` copy (`JTL-001:Foil`), one `sold` copy
(`SOR-130:Foil`), one user printing `SOR_193-user-1` (a misprint) held as a
copy, and a reconstructed value history (3 dates from `2025-06-13`, with the
`sealed` dimension non-zero). The ownership outbox is dated from its payloads.
