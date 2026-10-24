//! `dumpstar catalog-apply` — the SWU catalog apply job, run by a timer — and
//! `dumpstar catalog-check`, the alarm check run by another.
//!
//! Since sp-6i11rn the catalog is derived OFFLINE from the lake's `raw/`
//! (`dumpstar data refresh` lands, `dumpstar-lake-derive shared` derives). Since
//! sp-m1fuer the set art and sealed-product pictures come the same way: landed
//! in the lake (`source=swu-assets`) and replayed into the data directory by
//! the derive, so the prod container fetches no upstream at all.
//!
//! `--assets` is the legacy online picture fetch, kept as the rollback of that
//! cutover for a week and then deleted with the unit that ran it.
//!
//! `--swu` is the legacy online path — fetch → land → apply, against
//! `shared.sqlite`, the official API crawl (at most one a day) and then the
//! tcgcsv prices — kept runnable as the rollback of that cutover, until a
//! nightly cycle from the lake has been verified on prod. A dead official API
//! still lets prices through there; the job still exits non-zero for it.

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
    #[arg(long, conflicts_with_all = ["swu", "assets"])]
    fake: bool,
    /// The legacy online path: fetch the official catalog and the tcgcsv prices
    /// and apply them here. The catalog is derived from the lake now; this is
    /// the rollback.
    #[arg(long, conflicts_with = "assets")]
    swu: bool,
    /// The legacy online picture fetch: the set art and sealed-product pictures
    /// from the publishers' image hosts. The derive replays them from the lake
    /// now; this is the rollback. No catalog payload is fetched or applied.
    #[arg(long)]
    assets: bool,
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

/// The set art and the sealed-product pictures. A miss leaves the set showing
/// its name, so it is reported but does not fail the job; a set with no art at
/// all does. Returns what failed.
fn fetch_assets(
    http: &HttpFetch,
    image_dir: Option<PathBuf>,
    shared: &std::path::Path,
) -> anyhow::Result<Vec<&'static str>> {
    let mut failed = Vec::new();
    // The set logos and art crops are retrieved once into the data dir's own
    // store; no page ever reaches the publisher's CDN. A miss leaves the set
    // showing its name, so it is reported but does not fail the job.
    let art_dir = dumpstar_db::dumpstar_home()?.join("art");
    match dumpstar_catalog::art::fetch(
        &art_dir,
        http,
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
    let art_cov = dumpstar_catalog::art::coverage(&art_dir, &dumpstar_catalog::art::sources());
    println!("{}", art_cov.line());
    if !art_cov.missing.is_empty() {
        eprintln!("ALARM set-art: missing {}", art_cov.missing.join(", "));
        failed.push("set art");
    }
    let images = match image_dir {
        Some(p) => p,
        None => dumpstar_db::dumpstar_home()?.join("sealed-images"),
    };
    match Catalog::open_writer(shared)
        .map_err(anyhow::Error::from)
        .and_then(|c| {
            Ok(dumpstar_catalog::images::fetch_pending(
                &c,
                http,
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
    Ok(failed)
}

pub fn run(args: CatalogApplyArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.fake || args.swu || args.assets,
        "choose --assets (pictures), --swu (the legacy online catalog) or --fake"
    );
    let shared = shared_path(args.shared_db)?;
    let raw = match args.raw_dir {
        Some(p) => p,
        None => dumpstar_db::dumpstar_home()?.join("raw").join("swu"),
    };
    if args.fake {
        return run_source("fake", &FakeSource, &raw, &shared);
    }
    if args.assets {
        let http = HttpFetch::new(PAUSE)?;
        let failed = fetch_assets(&http, args.image_dir, &shared)?;
        anyhow::ensure!(failed.is_empty(), "failed: {}", failed.join(", "));
        return Ok(());
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
    failed.extend(fetch_assets(&http, args.image_dir, &shared)?);
    anyhow::ensure!(failed.is_empty(), "failed: {}", failed.join(", "));
    Ok(())
}

/// Print every open alarm and fail when there is one. A timer runs this with
/// `OnFailure=` the alert unit, so a skipped price day, a printing one upstream
/// lacks, or a thin price day pages.
pub fn check(args: CatalogCheckArgs) -> anyhow::Result<()> {
    let catalog = Catalog::open_readonly(&shared_path(args.shared_db)?)?;
    let report = check::run_report(catalog.conn(), Utc::now())?;
    let alarms = report.open;
    for c in check::set_coverage(catalog.conn())? {
        println!(
            "coverage {}: {}/{} ({}%)",
            c.set_code,
            c.linked,
            c.printings,
            c.percent()
        );
    }
    let mut accepted: std::collections::BTreeMap<&str, usize> = Default::default();
    for (a, _) in &report.accepted {
        *accepted.entry(a.kind.as_str()).or_default() += 1;
    }
    for (kind, n) in &accepted {
        println!("accepted {kind}: {n} (src/accepted_alarms.tsv)");
    }
    let mut info: std::collections::BTreeMap<&str, usize> = Default::default();
    for a in &report.info {
        *info.entry(a.kind.as_str()).or_default() += 1;
    }
    for (kind, n) in &info {
        println!("info {kind}: {n}");
    }
    for a in &alarms {
        println!("ALARM {} {}: {}", a.kind, a.key, a.detail);
    }
    let filings = dumpstar_db::dumpstar_home()?.join("filings");
    let findings = check::mechanic_findings(catalog.conn())?;
    match dumpstar_catalog::filing::file(&filings, &findings)? {
        Some(f) => println!(
            "filed {:?}: {} unmatched mechanic phrase(s) -> {}/outbox",
            f.action,
            f.findings.len(),
            filings.display()
        ),
        None => println!(
            "mechanic vocabulary: {} finding(s), none new",
            findings.len()
        ),
    }
    let art_dir = dumpstar_db::dumpstar_home()?.join("art");
    let art_cov = dumpstar_catalog::art::coverage(&art_dir, &dumpstar_catalog::art::sources());
    println!("{}", art_cov.line());
    for m in &art_cov.missing {
        println!("ALARM set-art {m}: listed in set_art.json, not in the art store");
    }
    anyhow::ensure!(
        alarms.is_empty() && art_cov.missing.is_empty(),
        "{} catalog alarm(s)",
        alarms.len() + art_cov.missing.len()
    );
    println!("catalog check: no alarms");
    Ok(())
}
