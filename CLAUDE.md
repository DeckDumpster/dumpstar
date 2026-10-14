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
| `crates/dumpstar-db` | SQLite schemas (`schema_shared.sql`, `schema_user.sql`), queries, seeds |
| `crates/dumpstar-catalog` | the SWU catalog seam: `schema.sql`, record types, raw landing, idempotent apply, the fake source, the `catalog-apply` job; the official-API and tcgcsv adapters (`swu`, `official`, `tcgcsv`), the join (`crosswalk`, `groups.tsv`), alarms (`check`, `catalog-check`) |
| `crates/dumpstar-ingest` | the TCGCSV client (category 79, Star Wars: Unlimited), retry, raw landing wire |
| `crates/dumpstar-lake`, `-lakehouse`, `-derive` | the raw landing zone, the offline derive, replay |
| `crates/dumpstar-keys`, `-ship`, `-erase` | tenant key custody, the shipper to the tenant zone, deletion |
| `crates/dumpstar-server`, `-cli` | the HTTP server and the `dumpstar` binary |
| `frontend/` | SvelteKit (Svelte 5, adapter-static). Tokens and ui primitives are dumpster-kit's, vendored at the revision `frontend/kit.lock` pins into `src/lib/kit/` (`scripts/kit-sync.sh <kit checkout>` bumps it; `tests/kit-pin.test.js` fails on drift); dumpstar's own theme is `src/lib/styles/theme.css` |
| `deploy/`, `Containerfile` | container image, units, `deploy/ci.sh` |
| `fixtures/swu/` | verbatim captures of the SWU data sources (official API, tcgcsv) |

**Not here yet** (each is its own bead of the dumpstar design): the SWU catalog
seam and variant model (a derive builds no card rows today), authentication and
tenant resolution under the dumpstar name, the shared `dumpster-kit`, and a
Star Wars: Unlimited UI fixture. `dumpstar seed-fixture` and `tests/ui` still
carry the Pokémon-flavoured test data they were copied with; it is test data
only and is replaced with the catalog seam.

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

## Names

Nothing here may carry the identity this workspace was copied from (its
environment prefix, its crate and unit prefix, its application name) or any of
the removed Pokémon modules (variant model, pokemontcg and Japan catalogs,
overrides, symbols, bundles, set discovery, standalone promos, the sub-type
map). `deploy/banned-names.sh` enforces it in CI (the `lint` tier) and
`tests/ci/banned_names_test.sh` proves the gate fails when it should.

## CI

`.github/workflows/ci.yml` provisions a throwaway VM through
`DeckDumpster/ephemeral-ci/provision@v2` (`spill: ec2`, `spill-after-seconds:
'60'`), runs `deploy/ci.sh` on it, and tears it down with `if: always()`. There
is deliberately **no `concurrency:` block** in that workflow: GitHub keeps one
pending run per group and evicts the rest, so runs queue before the VM spill can
act (deckdumpster sp-r4zvel). `tests/ci/workflow_test.sh` fails if one appears.
