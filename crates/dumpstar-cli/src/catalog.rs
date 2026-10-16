//! `dumpstar catalog-apply` — the SWU catalog apply job, run by a timer — and
//! `dumpstar catalog-check`, the alarm check run by another.
//!
//! Fetch → land → apply, against `shared.sqlite`. `--swu` is the live job: the
//! official API crawl (at most one a day) and then the tcgcsv prices. The two
//! run independently, so a dead official API still lets prices through; the job
//! still exits non-zero for it.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use clap::Args;
use dumpstar_catalog::fetch::HttpFetch;
use dumpstar_catalog::swu::{
    KEY_OFFICIAL_CRAWLED_ON, KEY_OFFICIAL_SINCE, KEY_TCGCSV_LAST_UPDATED, OFFICIAL_BASE,
    OfficialSource, TCGCSV_BASE, TcgcsvSource,
};
use dumpstar_catalog::{Catalog, CatalogSource, FakeSource, check, job};

/// The pause between two calls to an upstream.
const PAUSE: Duration = Duration::from_millis(250);

#[derive(Args)]
pub struct CatalogApplyArgs {
    /// Shared catalog database path (default: ~/.dumpstar/shared.sqlite).
    #[arg(long)]
    shared_db: Option<PathBuf>,
    /// Raw landing directory (default: ~/.dumpstar/raw/swu).
    #[arg(long)]
    raw_dir: Option<PathBuf>,
    /// Use the built-in hand-written fake catalog.
    #[arg(long, conflicts_with = "swu")]
    fake: bool,
    /// Fetch the official catalog and the tcgcsv prices.
    #[arg(long)]
    swu: bool,
    /// Origin of the official API (test-tier).
    #[arg(long, default_value = OFFICIAL_BASE)]
    official_base: String,
    /// Sealed-product image store (default: ~/.dumpstar/sealed-images).
    #[arg(long)]
    image_dir: Option<PathBuf>,
    /// Origin of tcgcsv (test-tier).
    #[arg(long, default_value = TCGCSV_BASE)]
    tcgcsv_base: String,
}

#[derive(Args)]
pub struct CatalogCheckArgs {
    /// Shared catalog database path (default: ~/.dumpstar/shared.sqlite).
    #[arg(long)]
    shared_db: Option<PathBuf>,
}

fn shared_path(p: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    Ok(match p {
        Some(p) => p,
        None => dumpstar_db::shared_db_path()?,
    })
}

fn run_source(
    name: &str,
    source: &dyn CatalogSource,
    raw: &std::path::Path,
    shared: &std::path::Path,
) -> anyhow::Result<()> {
    let reports = job::run_once(source, raw, shared, Utc::now())?;
    if reports.is_empty() {
        println!("{name}: nothing to fetch");
    }
    for r in reports {
        println!(
            "{name}: landed {} sha256={} applied={:?}",
            r.landed.path.display(),
            r.landed.provenance.sha256,
            r.applied
        );
    }
    print_coverage(shared, name == "tcgcsv")?;
    // Format rules are data from the retrieved rules documents, not a payload.
    let mut catalog = dumpstar_catalog::Catalog::open_writer(shared)?;
    println!("formats seeded: {} rows changed", catalog.seed_formats()?);
    Ok(())
}

/// Per-set tcgcsv id coverage and, after a price sync, every printing without
/// one with the reason the join left it.
fn print_coverage(shared: &std::path::Path, with_unmatched: bool) -> anyhow::Result<()> {
    let catalog = Catalog::open_readonly(shared)?;
    for c in check::set_coverage(catalog.conn())? {
        println!(
            "coverage {}: {}/{} ({}%)",
            c.set_code,
            c.linked,
            c.printings,
            c.percent()
        );
    }
    if with_unmatched {
        for line in check::unmatched(catalog.conn())? {
            println!("unmatched {line}");
        }
    }
    Ok(())
}

pub fn run(args: CatalogApplyArgs) -> anyhow::Result<()> {
    anyhow::ensure!(args.fake || args.swu, "choose --swu (live) or --fake");
    let shared = shared_path(args.shared_db)?;
    let raw = match args.raw_dir {
        Some(p) => p,
        None => dumpstar_db::dumpstar_home()?.join("raw").join("swu"),
    };
    if args.fake {
        return run_source("fake", &FakeSource, &raw, &shared);
    }
    if Catalog::open_writer(&shared)?.rejoin_if_stale()? {
        println!("crosswalk: the join changed since this catalog was joined; re-joined");
    }
    let reapplied = Catalog::open_writer(&shared)?.reapply_landed(&raw)?;
    if reapplied > 0 {
        println!("derivation changed: re-applied {reapplied} landed payloads");
    }
    let http = HttpFetch::new(PAUSE)?;
    let state = |key: &str| -> anyhow::Result<Option<String>> {
        Ok(Catalog::open_writer(&shared)?.sync_state(key)?)
    };
    let mut failed = Vec::new();
    let official = OfficialSource {
        http: &http,
        base: args.official_base,
        since: state(KEY_OFFICIAL_SINCE)?,
        crawled_on: state(KEY_OFFICIAL_CRAWLED_ON)?,
        today: Utc::now().format("%Y-%m-%d").to_string(),
    };
    if let Err(e) = run_source("official", &official, &raw, &shared) {
        eprintln!("official: {e:#}");
        failed.push("official");
    }
    let tcgcsv = TcgcsvSource {
        http: &http,
        base: args.tcgcsv_base,
        last_updated: state(KEY_TCGCSV_LAST_UPDATED)?,
    };
    if let Err(e) = run_source("tcgcsv", &tcgcsv, &raw, &shared) {
        eprintln!("tcgcsv: {e:#}");
        failed.push("tcgcsv");
    }
    // The set logos and art crops are retrieved once into the data dir's own
    // store; no page ever reaches the publisher's CDN. A miss leaves the set
    // showing its name, so it is reported but does not fail the job.
    let art_dir = dumpstar_db::dumpstar_home()?.join("art");
    match dumpstar_catalog::art::fetch(
        &art_dir,
        &http,
        Utc::now().date_naive(),
        &dumpstar_catalog::art::sources(),
    ) {
        Ok(r) => {
            println!("set art: fetched {} already {}", r.fetched, r.already);
            for f in r.failed {
                eprintln!("set art: {f}");
            }
        }
        Err(e) => eprintln!("set art: {e:#}"),
    }
    let images = match args.image_dir {
        Some(p) => p,
        None => dumpstar_db::dumpstar_home()?.join("sealed-images"),
    };
    match Catalog::open_writer(&shared)
        .map_err(anyhow::Error::from)
        .and_then(|c| {
            Ok(dumpstar_catalog::images::fetch_pending(
                &c,
                &http,
                &images,
                &Utc::now().format("%Y-%m-%d").to_string(),
            )?)
        }) {
        Ok(r) => {
            println!("{}", r.coverage_line());
            for f in &r.failed {
                eprintln!("sealed image: {f}");
            }
        }
        Err(e) => eprintln!("sealed images: {e:#}"),
    }
    anyhow::ensure!(failed.is_empty(), "failed: {}", failed.join(", "));
    Ok(())
}

/// Print every open alarm and fail when there is one. A timer runs this with
/// `OnFailure=` the alert unit, so a skipped price day, a printing one upstream
/// lacks, or a thin price day pages.
pub fn check(args: CatalogCheckArgs) -> anyhow::Result<()> {
    let catalog = Catalog::open_readonly(&shared_path(args.shared_db)?)?;
    let alarms = check::run(catalog.conn(), Utc::now())?;
    for c in check::set_coverage(catalog.conn())? {
        println!(
            "coverage {}: {}/{} ({}%)",
            c.set_code,
            c.linked,
            c.printings,
            c.percent()
        );
    }
    for a in &alarms {
        println!("ALARM {} {}: {}", a.kind, a.key, a.detail);
    }
    anyhow::ensure!(alarms.is_empty(), "{} catalog alarm(s)", alarms.len());
    println!("catalog check: no alarms");
    Ok(())
}
