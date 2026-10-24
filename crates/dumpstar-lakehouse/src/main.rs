//! `dumpstar-lake-derive` — the offline catalog derive.
//!
//! ```text
//!   dumpstar-lake-derive shared --ingest-date 2026-08-11 --db /data/shared.sqlite
//!   dumpstar-lake-derive diff --left a.sqlite --right b.sqlite --exclude raw_derivation
//! ```
//!
//! ## Why this is a separate binary
//!
//! > Only lakehouse code reads `raw/`. The shared and tenant databases are
//! > derived from that, so whatever produces them is **also** lakehouse code.
//!
//! That rule is how the offline and online processes decouple onto different
//! machines, and it is the reason this is a crate rather than a `--from-raw`
//! flag on `dumpstar data refresh`: a flag would put a raw reader inside
//! `dumpstar-cli`, on the **online** side, which is precisely the coupling the
//! rule exists to break. This crate is a binary with no library, so nothing
//! online can link it even by accident — and `dumpstar-cli` does not depend on
//! it, which is the mechanical version of the same statement.
//!
//! What it is **not** is a second derivation. The pipeline it runs is
//! [`dumpstar_derive::derive`], the body `dumpstar data refresh` used to run
//! inline, moved out of the CLI unchanged. All this binary supplies is where
//! the bytes come from ([`replay`]) and which bytes those are ([`partition`]).
//! That is what made item 3's comparison meaningful: two runs of ONE
//! derivation over two sources of the SAME bytes. Two implementations agreeing
//! would only ever have been evidence about the second implementation.
//!
//! Since item 6 (pd-lunn) the refresh calls [`dumpstar_derive::land`] — that
//! same acquisition, no imports — and this binary is `derive`'s only caller
//! left. So that comparison can no longer be run at all: there is no second
//! builder to diff against, which was the point of deleting one.
//!
//! ## What it refuses
//!
//! - a date whose partition was never landed, or landed incompletely — it
//!   never falls back to the newest available date, because "yesterday's raw
//!   silently deriving today's catalog" is the failure this whole design is
//!   arranged against;
//! - a partition whose manifests carry no clock, or disagree about it;
//! - a URL the partition has no record of. There is no fallback to the live
//!   upstream: a derive that fetched what `raw/` did not hold would produce a
//!   correct catalog whose lineage is not reproducible, which is the one
//!   failure the landing zone exists to prevent. The temporary fallback item 2
//!   shipped with, and its `--no-upstream-fallback` opt-out, are gone (item 4);
//! - `--ingest-date` defaulted from the wall clock. There is no default.
//!   Deriving an older date is the same operation as deriving today's, and a
//!   job that reads the clock has two behaviours where it should have one.
//!   The *scheduler* is the component allowed to know what day it is, so
//!   `deploy/derive.sh` names the date explicitly — exactly as
//!   `dumpstar-lake-value-snapshots` already requires.
//!
//! ## Exit status
//!
//! | | |
//! | --- | --- |
//! | **0** | the catalog is the derivation of that partition |
//! | **2** | the catalog was derived from the TCGCSV half alone: the official card crawl for that partition is absent or incomplete, so the card list is as old as the last whole crawl |
//! | **1** | there is no catalog for that partition — absent, short, clockless, a URL `raw/` has no record of, or the derivation itself failed |

mod replay;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use dumpster_kit_lake::LakeEnv;
use dumpster_kit_lakehouse::{diff, partition};

/// The offline half of the catalog: derive `shared.sqlite` from `raw/`.
#[derive(Parser)]
#[command(name = "dumpstar-lake-derive", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derive the shared catalog from one `raw/` partition.
    Shared(SharedArgs),
    /// Compare two catalogs row by row. The acceptance test's instrument.
    Diff(DiffArgs),
}

#[derive(clap::Args)]
struct SharedArgs {
    /// The `raw/` partition to derive from, `YYYY-MM-DD`.
    ///
    /// Required, with no default from the clock — see the module docs.
    #[arg(long, value_name = "YYYY-MM-DD")]
    ingest_date: String,

    /// The shared catalog to derive into (default: $DUMPSTAR_HOME/shared.sqlite).
    ///
    /// The derive is incremental, exactly as the online refresh is: it updates
    /// the catalog it is given rather than building one from nothing.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// The catalog's data directory. Defaults to the catalog's directory.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
struct DiffArgs {
    /// The catalog on the left of the comparison.
    #[arg(long, value_name = "PATH")]
    left: PathBuf,
    /// The catalog on the right.
    #[arg(long, value_name = "PATH")]
    right: PathBuf,
    /// A table to skip. Repeatable, and echoed in the report — an exclusion
    /// nobody can see is how a comparator starts proving nothing.
    #[arg(long, value_name = "TABLE")]
    exclude: Vec<String>,
    /// How many differing rows to print per table.
    #[arg(long, default_value_t = 5)]
    max_diffs: usize,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Shared(args) => shared(args),
        Command::Diff(args) => diff_cmd(args),
    }
}

/// Derive `shared.sqlite` from one raw partition.
fn shared(args: SharedArgs) -> anyhow::Result<()> {
    let db_path = match args.db {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    };
    let data_dir = args.data_dir.unwrap_or_else(|| {
        db_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    });

    // The lake, opened for READING. `open_reader` hands back a handle with no
    // `put` on it at all: `raw/` is immutable, and the job that derives from
    // the evidence must not be able to rewrite it.
    let zone = dumpster_kit_lake::open_reader(&LakeEnv::new(dumpstar_db::app::app()))?;
    println!(
        "Deriving ingest_date={} from {}",
        args.ingest_date,
        zone.describe()
    );

    // Which runs, and the refusals. Before anything opens the catalog: a date
    // that cannot be derived must not leave a half-written one behind.
    let mut plan = partition::choose(&zone, &args.ingest_date, &dumpstar_derive::TCGCSV_TABLE)?;
    // The official crawl is the half a night can do without: its absence costs
    // the card list its freshness, where a missing TCGCSV day costs prices that
    // cannot be fetched later. An incomplete crawl is treated as absent — half
    // a crawl is a quietly smaller catalog.
    let official = match partition::choose(
        &zone,
        &args.ingest_date,
        &dumpstar_derive::OFFICIAL_TABLE,
    ) {
        Ok(p) if !p.chosen.is_empty() => {
            plan.chosen.extend(p.chosen);
            true
        }
        Ok(_) => {
            eprintln!(
                "!! no official card crawl landed for {} — deriving the TCGCSV half alone",
                args.ingest_date
            );
            false
        }
        Err(e) => {
            eprintln!(
                "!! the official card crawl for {} is not usable ({e:#}) — deriving the TCGCSV half alone",
                args.ingest_date
            );
            false
        }
    };
    let clock = partition::clock_of(&plan.chosen, &args.ingest_date)?;
    println!(
        "  clock {} (observed {}) — recovered from the run's manifests, not read here",
        clock.fetched_at(),
        clock.observed_date()
    );

    let replay = Arc::new(replay::Replay(
        dumpster_kit_lakehouse::replay::RawReplay::new(zone, &plan.chosen)?,
    ));
    println!("  {} URL(s) replayable from this partition", replay.urls());

    println!("Opening shared catalog at {}", db_path.display());
    let mut conn = dumpstar_db::open_shared(&db_path)?;

    let report = dumpstar_derive::derive_with(
        &mut conn,
        &dumpstar_derive::Options {
            clock: clock.clone(),
            data_dir: &data_dir,
            // The offline job never lands. Landing is the other unit's job,
            // and a derive that re-landed what it just read would be writing
            // a second copy of its own input.
            landing: None,
            replay: Some(replay.clone() as Arc<dyn dumpstar_ingest::landing::ReplaySource>),
        },
        official,
    )?;

    // Provenance: `raw_derivation.complete` carries each dataset's own answer.
    // Not written when the run failed outright, because there is then no
    // catalog for it to describe.
    let rows: Vec<_> = partition::provenance(&plan.chosen, &args.ingest_date, &clock)
        .into_iter()
        .map(|p| dumpstar_db::raw_derivation::RawDerivation {
            ingest_date: p.ingest_date,
            source: p.source,
            dataset: p.dataset,
            run_id: p.run_id,
            parts: p.parts,
            complete: p.complete,
            observed_at: p.observed_at,
        })
        .collect();
    let derived_at = chrono::Utc::now().to_rfc3339();
    dumpstar_db::raw_derivation::record(&mut conn, &args.ingest_date, &derived_at, &rows)?;
    println!(
        "  provenance: {} partition(s) recorded in raw_derivation for {}",
        rows.len(),
        args.ingest_date
    );

    // Reaching here already means it: a URL outside the partition is a refusal
    // (see `replay`), so a run that finished was answered from `raw/` alone.
    // Said out loud anyway, because it is what an operator reads to know the
    // lineage is intact.
    println!("  raw coverage: complete — every upstream request was answered from raw/");

    println!(
        "Derive complete: {} ({} printings)",
        db_path.display(),
        report.printings
    );
    reclaim(&conn);
    if !official {
        // The catalog was rebuilt and is serving; its card list is as old as the
        // last night that had a whole crawl. Not a failure (the unit's
        // SuccessExitStatus=2), and not silent either: deploy/derive.sh says so.
        drop(conn);
        std::process::exit(EXIT_PARTIAL);
    }
    Ok(())
}

/// The catalog was derived from a partition with no usable official crawl.
const EXIT_PARTIAL: i32 = 2;

/// Give the catalog's WAL back, after this job's LAST write and before it exits
/// (pd-t50h).
///
/// The placement is the whole of the claim. A
/// checkpoint at the end of `dumpstar_derive::derive` would run before
/// `raw_derivation::record`, leaving the provenance row's frames in a file
/// nothing will ever truncate — an autocheckpoint runs on a commit, this
/// process is the only thing that commits to the catalog, and it is about to
/// exit. So the `-wal` a night leaves behind sits on the data volume until the
/// next night's run. `row_identical.rs::a_derive_leaves_no_wal_behind_on_the_
/// catalog_it_built` asserts the file is empty against the shipped binary, and
/// that assertion is what found the ordering.
///
/// Not fallible from here. `reclaim_catalog_wal` already reports a checkpoint a
/// reader blocked rather than raising — the catalog is complete either way, and
/// a browsing session must not be able to fail the nightly build — so the only
/// thing left to swallow is a checkpoint that could not run at all, which
/// changes nothing about the catalog this run just wrote and must not turn a
/// good derivation into exit 1.
fn reclaim(conn: &rusqlite::Connection) {
    if let Err(e) = dumpstar_db::reclaim_catalog_wal(conn) {
        eprintln!(
            "!! could not checkpoint the catalog's WAL: {e}. Disk only — the catalog is complete."
        );
    }
}

/// Compare two catalogs row by row.
fn diff_cmd(args: DiffArgs) -> anyhow::Result<()> {
    // Read-only, both of them: a comparison that could write is a comparison
    // that can change its own answer. `query_only` rather than SQLITE_OPEN_
    // READ_ONLY because these are WAL databases and opening one read-only
    // fails outright on a directory SQLite cannot create a -shm in.
    let left = open_query_only(&args.left)?;
    let right = open_query_only(&args.right)?;

    println!(
        "Comparing {} (left) with {} (right)",
        args.left.display(),
        args.right.display()
    );
    let report = diff::compare(&left, &right, &args.exclude, args.max_diffs)?;
    report.print();

    if report.matched() {
        println!("ROW-IDENTICAL: every compared table matches, row for row.");
        Ok(())
    } else {
        anyhow::bail!("the two catalogs are NOT row-identical — see the DIFF lines above")
    }
}

fn open_query_only(path: &std::path::Path) -> anyhow::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}
