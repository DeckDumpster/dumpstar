//! `dumpstar-derive` — how `shared.sqlite` is built, and the only copy of it.
//!
//! ## Why it is a crate rather than a function in the CLI
//!
//! > Only lakehouse code reads `raw/`. The shared and tenant databases are
//! > derived from that, so whatever produces them is **also** lakehouse code.
//!
//! That rule is how the offline and online processes decouple onto different
//! machines. It rules out a `--from-raw` flag on `dumpstar data refresh`,
//! because that puts a raw reader inside `dumpstar-cli`, on the **online**
//! side, which is the coupling the rule exists to break.
//!
//! So the derivation lives here, and this crate is both halves of a night:
//!
//! | entry point | caller | side | what it does |
//! | --- | --- | --- | --- |
//! | [`land`] | `dumpstar data refresh` (`dumpstar-cli`) | online | fetches every upstream into `raw/`, and derives nothing |
//! | [`derive`] | `dumpstar-lake-derive shared` (`dumpstar-lakehouse`) | offline | replays one `raw/` partition into `shared.sqlite` |
//!
//! **Nothing in this crate reads `raw/`.** It takes a
//! [`dumpstar_ingest::landing::Wire`], which is either empty, or writing
//! through to a landing zone, or answering from a
//! [`ReplaySource`](dumpstar_ingest::landing::ReplaySource) somebody else
//! implemented. The crate that implements one over `raw/` is
//! `dumpstar-lakehouse`, and it is a binary with no library, so no online
//! target can link it even by accident.
//!
//! ## What "the same rows twice" needs
//!
//! Two things, and both are inputs rather than ambient state:
//!
//! - **the clock** ([`DeriveClock`]) — read once by the landing side,
//!   recorded in the run's manifests, and read back by the deriving side. See
//!   `dumpster_kit_derive::clock`, which is the whole argument.
//! - **repo files** — `data/*.json` are compiled in (`include_str!`), so both
//!   sides of a comparison are running the same build's copy by construction.
//!
//! ## Where the catalog is
//!
//! The Pokemon catalog phases this crate was copied with are gone. The Star
//! Wars: Unlimited catalog arrives with the catalog seam (design row S6); until
//! then [`derive`] reconciles what the schema already seeds and refreshes the
//! materialised prices, and [`land`] fetches the TCGCSV Star Wars: Unlimited
//! category into `raw/`.

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use dumpstar_ingest::landing::{ReplaySource, Wire};
use dumpstar_ingest::tcgcsv::TcgcsvClient;
use dumpster_kit_lake::RawLanding;

// The clock and the landing bracket are the kit's; re-exported so the site's entry points
// (`land`, `derive`) and their callers name them from one place.
pub use dumpster_kit_derive::{DeriveClock, finalize_landing};

/// Everything a derivation needs that is not the database.
pub struct Options<'a> {
    /// The instant the inputs were fetched. See [`DeriveClock`].
    pub clock: DeriveClock,
    /// The catalog's data directory.
    pub data_dir: &'a Path,
    /// Land every upstream response before parsing it. `Some` on every
    /// [`land`] run — that is what a landing run is — and `None` on an offline
    /// [`derive`], which is reading a partition rather than writing one.
    pub landing: Option<Arc<RawLanding>>,
    /// Answer every upstream request from bytes already landed. `Some` is the
    /// offline derive; nothing online ever sets it.
    pub replay: Option<Arc<dyn ReplaySource>>,
}

impl Options<'_> {
    fn wire(&self) -> Wire {
        let mut wire = Wire::default();
        if let Some(landing) = &self.landing {
            wire = wire.landing_in(Arc::clone(landing));
        }
        if let Some(replay) = &self.replay {
            wire = wire.replaying(Arc::clone(replay));
        }
        wire
    }
}

/// What one derivation produced. Counts only.
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Rows written to `printings`.
    pub printings: usize,
}

/// Derive the shared catalog.
///
/// The acquisition phase is bracketed so the landing zone's manifests are
/// written whichever way it ends — a run that dies partway must leave a
/// manifest that says so, not a short prefix that reads as whole. Everything
/// after that bracket is local derivation, and a failure there says nothing
/// about whether the raw bytes arrived.
pub fn derive(conn: &mut Connection, options: &Options<'_>) -> anyhow::Result<Report> {
    let report = Report::default();

    // The search query language metadata seeded from data/search_*.json. The
    // read-write open this connection came from reconciled it
    // (connection.rs::converge), so it is read back rather than re-run.
    let sm = dumpstar_db::search_meta::counts(conn)?;
    println!(
        "  {} keywords, {} rarities, {} flags",
        sm.keywords, sm.rarities, sm.flags
    );

    let acquired = land_tcgcsv(options).map(|groups| println!("  {groups} group(s) walked"));
    if let Some(landing) = &options.landing {
        finalize_landing(landing, acquired.as_ref().err())?;
    }
    acquired?;

    // The derivation touches the SHARED catalog and nothing else — no tenant
    // database is opened, let alone written. tests/refresh/tenant_bytes.sh is
    // the gate: a refresh over a data directory with real tenant databases in
    // it must leave every one of them byte-identical.
    //
    // The WAL this run produced is given back by the CALLER, once its own last
    // write has landed — `dumpstar_db::reclaim_catalog_wal`. Not here:
    // `dumpstar-lake-derive shared` writes `raw_derivation` after this function
    // returns.
    Ok(report)
}

/// Fetch every upstream a derivation reads, land the bytes, and derive
/// **nothing** — the landing half of [`derive`] on its own.
///
/// This is what `dumpstar data refresh` runs. No catalog is opened, so no row
/// of one is written by this function or by anything it calls.
pub fn land(options: &Options<'_>) -> anyhow::Result<Report> {
    let report = Report::default();
    let acquired = land_tcgcsv(options).map(|groups| println!("  {groups} group(s) walked"));
    if let Some(landing) = &options.landing {
        finalize_landing(landing, acquired.as_ref().err())?;
    }
    acquired?;
    Ok(report)
}

/// Fetch every TCGCSV group's products and prices for the Star Wars:
/// Unlimited category.
fn land_tcgcsv(options: &Options<'_>) -> anyhow::Result<usize> {
    println!("Landing TCGCSV groups, products, prices...");
    let client = TcgcsvClient::new()?.on_wire(options.wire());
    let groups = client.fetch_groups()?;
    for group in &groups {
        client.fetch_products(group.group_id)?;
        client.fetch_prices(group.group_id)?;
    }
    Ok(groups.len())
}
