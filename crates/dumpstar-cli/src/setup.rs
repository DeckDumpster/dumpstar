//! `dumpstar setup` — create the shared catalog database.
//!
//! Opening the catalog read-write creates it and converges its schema and
//! shipped seeds ([`dumpstar_db::open_shared`]); a `shared.sqlite` written by
//! the Pokémon-shaped schema this project was forked with is emptied there
//! first. With `--land-raw` setup also
//! fetches the TCGCSV Star Wars: Unlimited category into the raw landing zone,
//! the same acquisition `dumpstar data refresh` runs nightly.
//!
//! The Star Wars: Unlimited catalog itself is filled by `dumpstar catalog-apply`
//! (`--swu` live, `--fake` for the test tiers); setup writes no card rows.

use std::path::PathBuf;

use dumpstar_derive::DeriveClock;

/// Arguments for `dumpstar setup`.
#[derive(clap::Args)]
pub struct SetupArgs {
    /// Shared catalog database path (default: ~/.dumpstar/shared.sqlite).
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// Land every upstream response in the raw landing zone before parsing
    /// it, under `raw/source=.../ingest_date=.../run=<ULID>/`.
    ///
    /// Requires ~/.config/dumpstar/lake.env to name the bucket; the command
    /// refuses to start without it rather than landing nothing quietly.
    /// Card art and set symbols are never landed. Also settable as
    /// DUMPSTAR_LAND_RAW=1.
    #[arg(long)]
    land_raw: bool,
}

/// Execute `dumpstar setup`.
pub fn run(args: SetupArgs) -> anyhow::Result<()> {
    let db_path = match args.db.clone() {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    };
    println!("Opening shared catalog at {}", db_path.display());
    let conn = dumpstar_db::open_shared(&db_path)?;

    // The run's clock, read ONCE — see `dumpstar_derive::clock`. It picks the
    // ingest_date partition and it is recorded in every manifest.
    let clock = DeriveClock::now();

    // Resolved before anything is fetched: a landing zone that was asked for
    // and is not configured should stop the run at the start, not after an
    // hour of requests whose bytes then have nowhere to go.
    let landing = crate::landing::open(args.land_raw, &clock)?;

    let data_dir = db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if landing.is_some() {
        dumpstar_derive::land(&dumpstar_derive::Options {
            clock,
            data_dir: &data_dir,
            landing,
            replay: None,
        })?;
    }

    // The search query language metadata (keywords, rarity ranks, is:-flag
    // definitions) seeded from data/search_*.json. The read-write open above
    // reconciled it (connection.rs::converge), so this reads the result back.
    let sm = dumpstar_db::search_meta::counts(&conn)?;
    println!(
        "  {} keywords, {} rarities, {} flags",
        sm.keywords, sm.rarities, sm.flags
    );

    // `setup` writes the catalog and then exits, and nothing that runs
    // afterwards will ever checkpoint what it left behind.
    dumpstar_db::reclaim_catalog_wal(&conn)?;

    println!("Setup complete: {}", db_path.display());
    Ok(())
}
