//! `dumpstar value-snapshots` — record a day's collection value for every
//! tenant (S12).
//!
//! The unit of work is the registry, not `$DUMPSTAR_USER`: each active tenant
//! is valued from its own copies and its own condition multipliers against the
//! shared catalog's price series, and written to that tenant's own file. It
//! opens no network, so it is an offline batch job.
//!
//! Exit status: 0 every tenant valued; 2 the run completed and at least one
//! tenant was skipped (named on stderr); 1 the run could not start.

use std::path::PathBuf;

use dumpstar_db::registry::UserState;
use dumpstar_db::value_history::{self, FleetTarget};

#[derive(clap::Args)]
pub struct ValueSnapshotsArgs {
    /// The `YYYY-MM-DD` to record. Required: a job that reads the clock has
    /// two behaviours where pointing it at an older date is the backfill.
    #[arg(long, value_name = "YYYY-MM-DD")]
    date: String,
    /// Shared catalog database path (default: ~/.dumpstar/shared.sqlite).
    #[arg(long, value_name = "PATH")]
    shared_db: Option<PathBuf>,
}

pub fn run(args: ValueSnapshotsArgs) -> anyhow::Result<()> {
    chrono::NaiveDate::parse_from_str(&args.date, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("--date must be YYYY-MM-DD, got {:?}: {e}", args.date))?;
    let shared = match args.shared_db {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    };
    if !shared.exists() {
        anyhow::bail!(
            "no shared catalog at {} — there are no prices to value anything with",
            shared.display()
        );
    }
    let targets: Vec<FleetTarget> = dumpstar_db::tenants::list()?
        .into_iter()
        .filter(|t| t.user.state == UserState::Active)
        .map(|t| FleetTarget {
            handle: t.user.handle,
            path: t.path,
        })
        .collect();
    if targets.is_empty() {
        anyhow::bail!("the registry holds no active tenant, so there is nothing to value");
    }

    let report = value_history::snapshot_fleet(&shared, &targets, &args.date);
    for (handle, rows) in &report.valued {
        println!("  {handle}: {rows} snapshot rows for {}", args.date);
    }
    for (handle, why) in &report.skipped {
        eprintln!("  {handle}: SKIPPED — {why}");
    }
    println!(
        "{} tenant(s) valued, {} skipped",
        report.valued.len(),
        report.skipped.len()
    );
    if !report.skipped.is_empty() {
        eprintln!(
            "warning: {} tenant(s) skipped: {}",
            report.skipped.len(),
            report
                .skipped
                .iter()
                .map(|(h, _)| h.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        std::process::exit(2);
    }
    Ok(())
}
