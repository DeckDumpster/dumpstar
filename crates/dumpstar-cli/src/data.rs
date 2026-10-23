//! `dumpstar data refresh` — the nightly LANDING run.
//!
//! It fetches every TCGCSV group's products and prices for Star Wars:
//! Unlimited, and writes each response into the `raw/` landing zone. It
//! builds nothing: the catalog has ONE builder, `dumpstar-lake-derive shared`,
//! which replays this partition some hours later.
//! `deploy/dumpstar-derive.timer` is what runs it, and on a box running this
//! command it is not optional.
//!
//! What is left in this file is the online half of a run: where the bytes go,
//! and the clock. **Nothing here reads `raw/`** — that is the boundary the
//! lakehouse epic turns on, and it is why the deriving job is a different
//! binary in a different crate rather than a `--from-raw` flag on this one.

use std::path::PathBuf;

/// The `dumpstar data` subcommand group.
#[derive(clap::Args)]
pub struct DataArgs {
    #[command(subcommand)]
    command: DataCommand,
}

#[derive(clap::Subcommand)]
enum DataCommand {
    /// Fetch every upstream and land it in `raw/`. Builds no catalog —
    /// `dumpstar-lake-derive shared` does that, from the partition this run
    /// lands.
    Refresh(RefreshCmdArgs),
    /// One-time: reconstruct collection value history from `shared.prices`
    /// × each copy's acquisition + status history, into the user DB's
    /// `collection_value_snapshot` table. `dumpstar-lake-value-snapshots`
    /// records today going forward, for every tenant; this seeds the past
    /// for the one `$DUMPSTAR_USER` names. Idempotent.
    BackfillValueHistory(RefreshArgs),
}

/// Arguments shared by the `dumpstar data` subcommands that only need a
/// database.
#[derive(clap::Args)]
pub struct RefreshArgs {
    /// Shared catalog database path (default: ~/.dumpstar/shared.sqlite).
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

/// Arguments for `dumpstar data refresh`.
///
/// **There is no `--land-raw`.** A run that does not land does nothing at all,
/// so landing is unconditional and `~/.config/dumpstar/lake.env` is required.
#[derive(clap::Args)]
pub struct RefreshCmdArgs {
    #[command(flatten)]
    common: RefreshArgs,
}

/// Execute `dumpstar data`.
pub fn run(args: DataArgs) -> anyhow::Result<()> {
    match args.command {
        DataCommand::Refresh(args) => refresh(args),
        DataCommand::BackfillValueHistory(args) => backfill_value_history(args),
    }
}

/// Execute `dumpstar data backfill-value-history` — a one-time reconstruction
/// of the collection's value over time. Unlike `refresh`/`setup` (which open
/// the *shared* catalog read-write), value snapshots live in the *user* DB,
/// so this opens a user connection (`connect_user`) with the shared catalog
/// attached read-only — collection value needs the user's copies.
fn backfill_value_history(args: RefreshArgs) -> anyhow::Result<()> {
    let shared_db = match args.db {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    };
    let user_db = crate::collection::user_db()?;
    println!(
        "Backfilling value history into {} (catalog {})",
        user_db.display(),
        shared_db.display()
    );
    let mut conn = dumpstar_db::connect_user(&user_db, &shared_db)?;
    let rows = dumpstar_db::value_history::backfill(&mut conn)?;
    println!("Value-history backfill complete: {rows} snapshot rows.");
    Ok(())
}

/// Execute `dumpstar data refresh` — fetch every upstream and LAND it.
///
/// It does not build the catalog: `dumpstar-lake-derive shared` is the
/// builder, and this command is the LANDING half and nothing else. Landing is
/// **required**, not a flag (see [`crate::landing::require`]), and the
/// derivation happens hours later, in its own unit, from the partition this
/// run landed.
///
/// Exit status: 0 when every upstream was acquired and landed, 1 when the run
/// failed. A TCGCSV pull that fails costs a day's prices permanently, so there
/// is no partial outcome.
fn refresh(args: RefreshCmdArgs) -> anyhow::Result<()> {
    let db_path = match args.common.db {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    };

    // The run's clock, read ONCE — see `dumpster_kit_derive::clock`. It picks the
    // ingest_date partition and it is recorded in every manifest, which is
    // what lets the offline derive stamp the same fetched_at / observed_at
    // into the same rows from these bytes.
    let clock = dumpstar_derive::DeriveClock::now();

    // Resolved before anything is fetched: a landing zone that cannot be
    // opened stops the run at the start, not after an hour of requests whose
    // bytes then have nowhere to go.
    let landing = crate::landing::require(&clock)?;

    let data_dir = db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    dumpstar_derive::land(&dumpstar_derive::Options {
        clock,
        data_dir: &data_dir,
        landing: Some(landing),
        // The online side never replays. It has no way to: reading the
        // landing zone is `dumpstar-lakehouse`'s job and this crate does
        // not depend on it.
        replay: None,
    })?;

    println!(
        "Refresh complete: landed, not derived. The catalog is built by dumpstar-lake-derive."
    );
    Ok(())
}

/// Raw-landing coverage: every upstream input a refresh consumes reaches
/// `raw/`.
///
/// This is a gate, not a review. A call-site audit cannot see a fetch that
/// skips `landing::fetch_bytes`, so the gate runs an acquisition against a fake
/// upstream and audits what was stored against what was asked for:
///
/// 1. every `Dataset` has a complete prefix with parts in it, walked from
///    [`Dataset::ALL`] so a dataset added later cannot be forgotten,
/// 2. every request the upstream served was landed, exactly once,
/// 3. the bytes are on disk under the keys the manifests claim.
#[cfg(test)]
mod raw_coverage {
    use std::sync::Arc;

    use dumpstar_ingest::test_upstream::{FakeUpstream, Reply};
    use dumpstar_ingest::upstream::{ENV_OFFICIAL_BASE_URL, ENV_TCGCSV_BASE_URL};
    use dumpster_kit_lake::{Dataset, DirStore, RawLanding};

    const INGEST_DATE: &str = "2026-09-20";
    const CLOCK_AT: &str = "2026-09-20T06:00:00Z";

    const GROUPS: &str = r#"{"results":[
        {"groupId":23237,"name":"Spark of Rebellion","abbreviation":"SOR",
         "publishedOn":"2024-03-08"}],"success":true,"errors":[]}"#;
    const EMPTY: &str = r#"{"results":[],"success":true,"errors":[]}"#;

    fn route(target: &str, _n: usize) -> Reply {
        let path = target.split('?').next().unwrap_or(target);
        match path {
            "/api/cards" => Reply::ok(r#"{"data":[],"meta":{"pagination":{"pageCount":1}}}"#),
            "/last-updated.txt" => Reply::ok("2026-09-19T20:05:44+0000"),
            "/tcgplayer/79/groups" => Reply::ok(GROUPS),
            p if p.ends_with("/products") || p.ends_with("/prices") => Reply::ok(EMPTY),
            other => Reply::status(
                404,
                format!(
                    r#"{{"error":"the acquisition phase asked for {other}, which this \
                        fixture does not model — a new upstream call needs a route here \
                        AND a landed dataset"}}"#
                ),
            ),
        }
    }

    /// The gate. See the module docs for what each assertion is for.
    #[test]
    fn a_night_lands_every_dataset_the_catalog_is_derived_from() {
        let upstream = FakeUpstream::start(route);
        // crate::ENV_LOCK is shared with every module that sets upstream origins.
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the lock is held for as long as the variable is set.
        unsafe {
            std::env::set_var(ENV_TCGCSV_BASE_URL, upstream.base_url());
            std::env::set_var(ENV_OFFICIAL_BASE_URL, upstream.base_url());
        }

        let tmp = tempfile::tempdir().unwrap();
        let clock = dumpstar_derive::DeriveClock::from_manifest(CLOCK_AT, "test clock")
            .expect("parse clock");
        let landing = Arc::new(RawLanding::new(
            Box::new(DirStore::new(tmp.path())),
            INGEST_DATE,
            clock.fetched_at(),
        ));
        let outcome = dumpstar_derive::land(&dumpstar_derive::Options {
            clock,
            data_dir: tmp.path(),
            landing: Some(Arc::clone(&landing)),
            replay: None,
        });
        // SAFETY: as above.
        unsafe {
            std::env::remove_var(ENV_TCGCSV_BASE_URL);
            std::env::remove_var(ENV_OFFICIAL_BASE_URL);
        }
        outcome.expect("the landing run");
        let manifests = landing.manifests();

        // 1. Every dataset: the kit's TCGCSV three, walked from its list rather
        // than one written here, and the two the catalog adds.
        let every: Vec<Dataset> = Dataset::ALL
            .iter()
            .copied()
            .chain([dumpstar_derive::CARDS, dumpstar_derive::LAST_UPDATED])
            .collect();
        for dataset in &every {
            let landed = manifests
                .iter()
                .find(|m| m.dataset == dataset.as_str())
                .unwrap_or_else(|| panic!("a night landed no {dataset} at all"));
            assert!(
                !landed.parts.is_empty(),
                "{dataset} has a prefix but no parts in it"
            );
            assert!(landed.complete, "{dataset} landed an incomplete run");
        }

        // 2. Everything asked for was stored, compared rather than read.
        let strip = |u: &String| {
            u.strip_prefix(&upstream.base_url())
                .unwrap_or(u)
                .to_string()
        };
        let mut served: Vec<String> = upstream.requests().iter().map(strip).collect();
        served.sort();
        let mut stored: Vec<String> = manifests
            .iter()
            .flat_map(|m| m.parts.iter().map(|p| strip(&p.url)))
            .collect();
        stored.sort();
        assert_eq!(
            served, stored,
            "every upstream response a refresh receives must be landed, exactly once"
        );

        // 3. The bytes are on disk under the keys the manifests claim.
        for m in &manifests {
            for part in &m.parts {
                assert!(
                    tmp.path().join(&part.key).is_file(),
                    "{} is in the manifest but not in the store",
                    part.key
                );
            }
        }
    }
}
