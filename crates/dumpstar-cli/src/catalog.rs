//! `dumpstar catalog-apply` — the SWU catalog apply job, run by a timer.
//!
//! Fetch → land → apply, against `shared.sqlite`. Only the fake source exists
//! until the official-API and tcgcsv adapters land (design row S7).

use std::path::PathBuf;

use clap::Args;
use dumpstar_catalog::{FakeSource, job};

#[derive(Args)]
pub struct CatalogApplyArgs {
    /// Shared catalog database path (default: ~/.dumpstar/shared.sqlite).
    #[arg(long)]
    shared_db: Option<PathBuf>,
    /// Raw landing directory (default: ~/.dumpstar/raw/swu).
    #[arg(long)]
    raw_dir: Option<PathBuf>,
    /// Use the built-in hand-written fake catalog.
    #[arg(long)]
    fake: bool,
}

pub fn run(args: CatalogApplyArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.fake,
        "only --fake is available until the SWU source adapters land"
    );
    let shared = match args.shared_db {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    };
    let raw = match args.raw_dir {
        Some(p) => p,
        None => dumpstar_db::dumpstar_home()?.join("raw").join("swu"),
    };
    for r in job::run_once(&FakeSource, &raw, &shared, chrono::Utc::now())? {
        println!(
            "landed {} sha256={} applied={:?}",
            r.landed.path.display(),
            r.landed.provenance.sha256,
            r.applied
        );
    }
    Ok(())
}
