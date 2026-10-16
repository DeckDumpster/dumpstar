# Dumpstar — instructions for agents

Dumpstar is the Star Wars: Unlimited collection site (`star.dumpster.cards`): a
multi-tenant Rust server and SvelteKit frontend, a sibling of deckdumpster and
the Pokémon site. This repository began as a copy of that site's workspace with
the Pokémon-specific modules removed (design row S1); the Star Wars: Unlimited
catalog, tenancy and the shared kit arrive in later rows.

## What is here, and what is not yet

| | |
| --- | --- |
| `crates/dumpstar-core` | domain types, the collection search query parser, importers |
| `crates/dumpstar-db` | SQLite schemas — `shared.sqlite` is `dumpstar-catalog`'s `schema.sql` plus the overlay `schema_shared.sql`, tenants are `schema_user.sql` — and the queries and seeds over them |
| `crates/dumpstar-catalog` | the SWU catalog seam: `schema.sql`, record types, raw landing, idempotent apply, the fake source, the `catalog-apply` job; the official-API and tcgcsv adapters (`swu`, `official`, `tcgcsv`), the join (`crosswalk`, `groups.tsv`), alarms (`check`, `catalog-check`) |
| `crates/dumpstar-ingest` | the transport for the kit's TCGCSV client (`dumpster-kit-tcgcsv`, pinned by rev in the workspace `Cargo.toml`; category 79, Star Wars: Unlimited), retry, raw landing wire |
| `crates/dumpstar-derive`, `-lakehouse` | the site's land/derive entry points and the `dumpstar-lake-derive` binary, over `dumpster-kit-lake`/`-derive`/`-lakehouse` |
| `crates/dumpstar-ship`, `-erase` | the `DUMPSTAR` bindings and operator binaries over the kit's `-keys`, `-ship`, `-erase`; the raw landing zone, key custody, shipper, deletion and replay themselves are the kit's (`dumpster-kit-*` pinned in the workspace `Cargo.toml`) |
| `crates/dumpstar-server`, `-cli` | the HTTP server and the `dumpstar` binary |
| `frontend/` | SvelteKit (Svelte 5, adapter-static). Tokens and ui primitives are dumpster-kit's, vendored at the revision `frontend/kit.lock` pins into `src/lib/kit/` (`scripts/kit-sync.sh <kit checkout>` bumps it; `tests/kit-pin.test.js` fails on drift); dumpstar's own theme is `src/lib/styles/theme.css` |
| `deploy/`, `Containerfile` | container image, units, `deploy/ci.sh` |
| `fixtures/swu/` | verbatim captures of the SWU data sources (official API, tcgcsv) and, in `rules/`, the retrieved Comprehensive Rules the format data cites |

**One card model.** `shared.sqlite` holds the Star Wars: Unlimited catalog and
nothing else: `dumpstar-catalog/src/schema.sql` (sets, cards, printings, the
crosswalk, prices, formats, the FTS index) with `dumpstar-db`'s overlay on top
(search registry, sealed products). A card belongs to no set; a *printing*
carries set, collector number, variant (Standard, Hyperspace, Showcase, Promo)
and finish (Normal, Foil), and (set, number, finish) names one. Collection,
deck, binder, wishlist and sealed rows reference `printing_id` / `card_id` from
it. A `shared.sqlite` written by the Pokémon-shaped schema this repository was
forked with is detected (`dumpstar_catalog::reset_if_legacy`) and emptied on the
next read-write open or `catalog-apply`; a column the schema merely gains is added in place (`ADDITIVE_COLUMNS`), never by a reset (sp-0mjd2y), and a deploy fails and restores if sets/cards/printings counts change by more than 5%. The legacy file is derived data, tenant files are
other files and are never touched.

**Not here yet** (each is its own bead of the dumpstar design): authentication
and tenant resolution under the dumpstar name, the shared `dumpster-kit`
adoption beyond tokens and primitives, and a Star Wars: Unlimited rework of the
`tests/ui` intents (`dumpstar seed-fixture` builds
an SWU fixture; those still name the Pokémon cards they were written for
and are outside CI). Sealed products are filled from tcgcsv's numberless
products at convergence; their daily prices are `sealed_prices`, written by the
catalog apply job as a series apart from the card `prices`. Their pictures are
tcgcsv's `imageUrl`, recorded in `sealed_images` and fetched by `catalog-apply`
into `$DUMPSTAR_HOME/sealed-images/`, served at `/sealed-img/`; pages never see
the source URL. A product tcgcsv lacks is a row of `data/sealed-remedial.json`.

## Build and test

```bash
cargo build
cargo test -p dumpstar-db          # one crate; `cargo test $(work crates)` for a diff
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cd frontend && npm ci && npm test && npm run check && npm run build
```

`cargo test` regenerates the TypeScript types in `frontend/src/lib/types/`
through ts-rs (`.cargo/config.toml`).

Container gates run through the repository's own tooling, never against a
production instance: `bash deploy/setup.sh <name> --test` with any name except
`prod`. `deploy/ci.sh` is what CI runs; `deploy/ci-select.sh` is its tier list.

`$DUMPSTAR_HOME` overrides the data dir (default `~/.dumpstar/`).
`$DUMPSTAR_USER` overrides the active tenant (default `collection`).

## Isolation gate

`DUMPSTAR_CI_TIERS=isolation` (tier `isolation` in `deploy/ci.sh`) runs
`tests::isolation` in `crates/dumpstar-server/src/lib.rs`: cross-tenant reads and
deletes, the leftover tenant header, the ratchet that reads every `.route(` in
`src/routes/` and demands a 401 without a token (public routes are listed in
`PUBLIC_API_ROUTES`, a deliberate edit), and `Cache-Control: private, no-store`
on every `/api` response. A new test belongs in that module so the tier selects it.

## Before you submit

Run the CI tiers your change touches, before `work submit`:
`testenv --suites <lint|rust|isolation|deploy|frontend|container> spira/<bead> dumpstar`
(declared in `.spira/testenv.toml`, mirroring `deploy/ci.sh`; with no `--suites` it runs `lint` and `rust`).
Commit first: it tests the committed branch.

## Copy

Every string a user can read is written for Star Wars: Unlimited, from the
game's own vocabulary and cards in the catalog (sets, leaders, bases, units,
events, upgrades, aspects, arenas, Hyperspace and Showcase treatments, booster
boxes and two-player starters). There is no deny-list check; write the copy
right the first time.

## CI

`.github/workflows/ci.yml` provisions a throwaway VM through
`DeckDumpster/ephemeral-ci/provision@v2` (`spill: ec2`, `spill-after-seconds:
'60'`), runs `deploy/ci.sh` on it, and tears it down with `if: always()`. There
is deliberately **no `concurrency:` block** in that workflow: GitHub keeps one
pending run per group and evicts the rest, so runs queue before the VM spill can
act (deckdumpster sp-r4zvel). `tests/ci/workflow_test.sh` fails if one appears.

## Use-case coverage

The use-case catalogue is `tests/ui/intents/*.yaml` (id on line 2: `# Use case: UC-<area>-NN`);
`docs/test-plan/*.toml` and `coverage.{json,md}` are generated from it (`bash tests/ci/plan.sh
generate|report`). Every shell suite under `tests/` carries `# tier:` and `# covers:` headers, and a
replay implementation carries `// covers: <its intent's UC id>`. The lint tier runs `tests/ci/plan.sh
lint` and `orphans`: a use case with no covering suite, or whose last cover a change deleted, fails by
name. A new intent needs `plan.sh assign` then `generate` and `report`. The binary is Spira's
`test-plan` (`$SPIRA_TEST_PLAN_BIN`, or on PATH).
