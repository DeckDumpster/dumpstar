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
//! The Star Wars: Unlimited catalog is `dumpstar-catalog`'s: the official card
//! API crawl and the TCGCSV groups, products, prices and `last-updated.txt`
//! stamp. [`land`] fetches all of it into `raw/` (`source=official
//! dataset=cards`, `source=tcgcsv dataset=groups|products|prices|last-updated`);
//! [`derive`] replays it through the **same** [`OfficialSource`] and
//! [`TcgcsvSource`] adapters, and applies every payload with the same
//! `dumpstar_catalog::apply` the online `catalog-apply --swu` used to run
//! (sp-6i11rn: relocated, not rewritten). The adapters only ever fetch through
//! a [`WireFetch`], so landing and replaying are two settings of one function.
//!
//! The landing crawls the official API **in full** every night, without the
//! `updated_since` resume point the online job kept in `shared.sqlite`: the
//! landing unit has no catalog to read it from, and applying an unchanged page
//! is skipped by its content hash, so a full crawl derives the rows an
//! incremental one would.
//!
//! A night whose official crawl did not complete still derives the TCGCSV half
//! (a day's prices cannot be fetched later): see [`derive_with`].

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use dumpstar_catalog::CatalogError;
use dumpstar_catalog::apply::apply;
use dumpstar_catalog::fetch::{Fetch, USER_AGENT};
use dumpstar_catalog::landing::{Provenance, sha256_hex};
use dumpstar_catalog::swu::{OFFICIAL_BASE, OfficialSource, TCGCSV_BASE, TcgcsvSource};
use dumpstar_catalog::{Catalog, CatalogSource};
use dumpstar_ingest::landing::{self, ReplaySource, Wire};
use dumpstar_ingest::upstream;
use dumpster_kit_lake::{Dataset, PartFormat, RawLanding, Source};
use dumpster_kit_lakehouse::partition::{Incomplete, Need, Requirement, Requirements};

// The clock and the landing bracket are the kit's; re-exported so the site's entry points
// (`land`, `derive`) and their callers name them from one place.
pub use dumpster_kit_derive::{DeriveClock, finalize_landing};

/// `source=official` — the official Star Wars: Unlimited card API.
pub const OFFICIAL: Source = Source::new("official");
/// `dataset=cards` — one part per page of `/api/cards`.
pub const CARDS: Dataset = Dataset::new("cards");
/// `dataset=last-updated` — tcgcsv's `last-updated.txt`, whose day is the day
/// every price payload of the run describes.
pub const LAST_UPDATED: Dataset = Dataset::new("last-updated");

/// `source=swu-assets` — the pictures the site serves itself: set logos and
/// art crops, and sealed-product images, fetched from the publishers' image
/// hosts by the landing unit (sp-m1fuer). The online container fetches none.
pub const ASSETS: Source = Source::new("swu-assets");
/// `dataset=set-art` — one binary part per listed set logo or art crop.
pub const SET_ART: Dataset = Dataset::new("set-art");
/// `dataset=sealed-images` — one binary part per sealed product's picture.
pub const SEALED_IMAGES: Dataset = Dataset::new("sealed-images");

/// What a derive needs of the pictures: neither. A set with no art shows its
/// name and a sealed product shows no picture, and `catalog-check` alarms both;
/// a night whose picture hosts were down must not cost a catalog. An incomplete
/// run is read as far as it goes — a picture it lacks is reported per asset.
pub const ASSETS_TABLE: Requirements<'static> = Requirements {
    rows: &[
        Requirement {
            source: ASSETS,
            dataset: SET_ART,
            need: Need::Optional,
        },
        Requirement {
            source: ASSETS,
            dataset: SEALED_IMAGES,
            need: Need::Optional,
        },
    ],
    incomplete: Incomplete::Partial,
};

/// What a derive needs of the TCGCSV half: all of it, an incomplete run a
/// refusal. A price is a fact about one day that there is no asking for later.
pub const TCGCSV_TABLE: Requirements<'static> = Requirements {
    rows: &[
        Requirement {
            source: Source::TCGCSV,
            dataset: Dataset::GROUPS,
            need: Need::Required,
        },
        Requirement {
            source: Source::TCGCSV,
            dataset: Dataset::PRODUCTS,
            need: Need::Required,
        },
        Requirement {
            source: Source::TCGCSV,
            dataset: Dataset::PRICES,
            need: Need::Required,
        },
        Requirement {
            source: Source::TCGCSV,
            dataset: LAST_UPDATED,
            need: Need::Required,
        },
    ],
    incomplete: Incomplete::Refuse,
};

/// What a derive needs of the official crawl: optional, because a dead
/// official API must not cost a night's prices. Absent or incomplete, the night
/// derives without it and the job says so (exit 2 of `dumpstar-lake-derive`).
pub const OFFICIAL_TABLE: Requirements<'static> = Requirements {
    rows: &[Requirement {
        source: OFFICIAL,
        dataset: CARDS,
        need: Need::Optional,
    }],
    incomplete: Incomplete::Refuse,
};

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
    /// Rows in `printings` once the derive finished.
    pub printings: usize,
    /// Payloads applied this run (not counting those already applied).
    pub applied: usize,
    /// Payloads whose content was already applied and so skipped.
    pub skipped: usize,
    /// Pictures stored into the data directory this run (set art, then sealed
    /// products); empty when the partition held no pictures.
    pub pictures: Pictures,
}

/// What replaying a partition's pictures into the data directory did.
#[derive(Debug, Default, Clone)]
pub struct Pictures {
    /// Whether the partition held any picture dataset at all.
    pub present: bool,
    pub set_art_fetched: usize,
    pub set_art_already: usize,
    pub sealed_stored: usize,
    /// One line per asset the partition could not supply.
    pub failed: Vec<String>,
}

/// The one door to the network for the catalog's adapters: a
/// [`dumpstar_catalog::fetch::Fetch`] that goes through
/// [`landing::fetch_bytes`], so retry, raw landing and replay apply to every
/// request the official and TCGCSV sources make. A replaying wire reaches no
/// upstream at all.
pub struct WireFetch {
    http: reqwest::blocking::Client,
    wire: Wire,
    /// The dataset a URL that is no catalog payload is a picture of, when this
    /// fetch is for pictures.
    pictures: Option<Dataset>,
    /// [`upstream::ENV_PICTURE_BASE_URL`], when set: the origin picture URLs are
    /// requested from (and replayed under) in place of the publishers' own.
    picture_base: Option<String>,
}

impl WireFetch {
    pub fn new(wire: Wire) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
            wire,
            pictures: None,
            picture_base: None,
        })
    }

    /// Also answer picture URLs, landing them under `dataset` of
    /// [`ASSETS`].
    pub fn for_pictures(mut self, dataset: Dataset) -> Self {
        self.pictures = Some(dataset);
        self.picture_base = picture_base();
        self
    }

    /// The URL a picture is requested by: the publisher's, or the same path
    /// under the test-tier origin. Landing and replaying go through this one
    /// function, so both key the same URL.
    fn picture_url(&self, url: &str) -> String {
        let Some(base) = &self.picture_base else {
            return url.to_string();
        };
        let path = url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("");
        format!("{base}{path}")
    }
}

/// The test-tier override of the picture hosts, announced like the other
/// origins; `None` — the only way prod runs — leaves every URL as listed.
fn picture_base() -> Option<String> {
    std::env::var(upstream::ENV_PICTURE_BASE_URL)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|_| {
            upstream::base_url(
                upstream::ENV_PICTURE_BASE_URL,
                "the publishers' image hosts",
            )
        })
}

/// Whether the night's pictures are landed. Always, unless the catalog's
/// upstreams are pointed at fixtures and the pictures are not: the listed
/// picture hosts are the real internet, which no fixture run may reach.
fn land_pictures_enabled() -> bool {
    let set = |k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty());
    set(upstream::ENV_PICTURE_BASE_URL)
        || !(set(upstream::ENV_TCGCSV_BASE_URL) || set(upstream::ENV_OFFICIAL_BASE_URL))
}

/// The media type of a picture, by its URL's extension. Landing names the part
/// by it; a replay looks the part up by URL and does not care.
fn picture_format(url: &str) -> PartFormat {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    PartFormat::Bin(match path.rsplit_once('.').map(|(_, e)| e) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    })
}

/// Which landed dataset a catalog URL belongs to.
fn dataset_of(url: &str) -> Option<(Source, Dataset, PartFormat)> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('/').next()? {
        "cards" if path.contains("/api/") => Some((OFFICIAL, CARDS, PartFormat::Json)),
        "groups" => Some((Source::TCGCSV, Dataset::GROUPS, PartFormat::Json)),
        "products" => Some((Source::TCGCSV, Dataset::PRODUCTS, PartFormat::Json)),
        "prices" => Some((Source::TCGCSV, Dataset::PRICES, PartFormat::Json)),
        // A one-line text file; the kit's part formats have no plain text, and
        // the extension is the only thing that differs.
        "last-updated.txt" => Some((Source::TCGCSV, LAST_UPDATED, PartFormat::Csv)),
        _ => None,
    }
}

impl Fetch for WireFetch {
    fn get(&self, url: &str) -> dumpstar_catalog::Result<Vec<u8>> {
        let (source, dataset, format) = dataset_of(url)
            .or_else(|| {
                self.pictures
                    .map(|dataset| (ASSETS, dataset, picture_format(url)))
            })
            .ok_or_else(|| CatalogError::Source(format!("{url}: not a landed catalog dataset")))?;
        // A replayed response reaches no upstream, so there is nothing to be
        // polite to.
        if !self.wire.is_replaying() {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let url = &self.picture_url(url);
        landing::fetch_bytes(
            &self.http,
            self.http.get(url),
            &self.wire,
            source,
            dataset,
            format,
        )
        .map_err(|e| CatalogError::Source(format!("{url}: {e}")))
    }
}

/// The origins the catalog's two upstreams are read from, and — because they
/// are the key a replay looks a URL up by — the same on both sides of a
/// comparison. Unset, which is the only way prod runs, they are the constants.
fn origins() -> (String, String) {
    (
        upstream::base_url(upstream::ENV_OFFICIAL_BASE_URL, OFFICIAL_BASE),
        upstream::base_url(upstream::ENV_TCGCSV_BASE_URL, TCGCSV_BASE),
    )
}

/// The payloads of one night, in apply order: the official crawl (its pages,
/// then the crawl marker) when `official`, then TCGCSV (groups, then each
/// group's products and prices, then the stamp).
///
/// Fetching is the adapters' own, unchanged; whether the bytes come from the
/// network or from `raw/` is the wire's.
fn payloads(
    options: &Options<'_>,
    official: bool,
) -> anyhow::Result<(Vec<dumpstar_catalog::RawPayload>, Option<anyhow::Error>)> {
    let (official_base, tcgcsv_base) = origins();
    let fetch = WireFetch::new(options.wire())?;
    let mut out = Vec::new();
    let mut official_err = None;

    // The apply order of the online job this replaced: cards, then prices.
    if official {
        println!("Replaying the official card pages...");
        match (OfficialSource {
            http: &fetch,
            base: official_base,
            since: None,
            crawled_on: None,
            today: options.clock.observed_date().to_string(),
        })
        .fetch()
        {
            Ok(p) => out.extend(p),
            Err(e) => official_err = Some(anyhow::Error::new(e)),
        }
    }
    println!("Replaying TCGCSV groups, products, prices...");
    out.extend(
        TcgcsvSource {
            http: &fetch,
            base: tcgcsv_base,
            last_updated: None,
        }
        .fetch()?,
    );
    Ok((out, official_err))
}

/// Derive the shared catalog from the partition `options` replays.
///
/// Equivalent to [`derive_with`] with the official crawl required and no
/// pictures.
pub fn derive(conn: &mut Connection, options: &Options<'_>) -> anyhow::Result<Report> {
    derive_with(conn, options, true, false)
}

/// Derive the shared catalog; `official` says whether the partition holds the
/// official crawl. Without it the night derives the TCGCSV half alone — the
/// catalog's cards are as old as the last night that had one — and the caller
/// reports that.
///
/// `pictures` says whether the partition holds [`ASSETS`]: when it does, the
/// set art and the sealed-product pictures are replayed from it into
/// `<data_dir>/art` and `<data_dir>/sealed-images`, where the server serves them
/// — the online container fetches none (sp-m1fuer). A picture the partition
/// lacks is reported in [`Report::pictures`], never a failed derive.
///
/// The derivation touches the SHARED catalog and nothing else — no tenant
/// database is opened, let alone written. tests/refresh/tenant_bytes.sh is the
/// gate.
///
/// The WAL this run produced is given back by the CALLER, once its own last
/// write has landed — `dumpstar_db::reclaim_catalog_wal`. Not here:
/// `dumpstar-lake-derive shared` writes `raw_derivation` after this function
/// returns.
pub fn derive_with(
    conn: &mut Connection,
    options: &Options<'_>,
    official: bool,
    pictures: bool,
) -> anyhow::Result<Report> {
    // The search query language metadata seeded from data/search_*.json. The
    // read-write open this connection came from reconciled it
    // (connection.rs::converge), so it is read back rather than re-run.
    let sm = dumpstar_db::search_meta::counts(conn)?;
    println!(
        "  {} keywords, {} rarities, {} flags",
        sm.keywords, sm.rarities, sm.flags
    );

    // The acquisition phase is bracketed so the landing zone's manifests are
    // written whichever way it ends (an online derive that lands as it goes).
    // Everything after is local derivation, and a failure there says nothing
    // about whether the raw bytes arrived.
    let fetched = payloads(options, official);
    if let Some(landing) = &options.landing {
        let failure = match &fetched {
            Err(e) => Some(e),
            Ok((_, e)) => e.as_ref(),
        };
        finalize_landing(landing, failure)?;
    }
    let (payloads, official_err) = fetched?;
    if let Some(e) = official_err {
        // The crawl was in the partition and could not be replayed in full:
        // applying half of it would be a quietly smaller catalog.
        return Err(e.context("the official crawl is not replayable from this partition"));
    }

    // The adapters apply through a `Catalog`, which owns its connection. Lend it
    // ours and take it back whatever happens.
    let owned = std::mem::replace(conn, Connection::open_in_memory()?);
    let mut catalog = Catalog::from_connection(owned);
    let result = apply_all(&mut catalog, options, &payloads).and_then(|mut report| {
        if pictures {
            report.pictures = replay_pictures(&catalog, options)?;
        }
        Ok(report)
    });
    *conn = catalog.into_connection();
    let mut report = result?;
    // What the open converged before a single payload existed, converged again
    // over what this run applied: the sealed products live in `tcgcsv_products`.
    dumpstar_db::converge_shared(conn)?;
    report.printings =
        conn.query_row("SELECT count(*) FROM printings", [], |r| r.get::<_, i64>(0))? as usize;
    Ok(report)
}

/// Replay the partition's pictures into the data directory through the same
/// stores the online `catalog-apply --assets` filled: [`dumpstar_catalog::art`]
/// and [`dumpstar_catalog::images`], over a [`WireFetch`] that can only answer
/// from `raw/`. Which sealed pictures are wanted is the `sealed_images` rows the
/// apply just wrote.
fn replay_pictures(catalog: &Catalog, options: &Options<'_>) -> anyhow::Result<Pictures> {
    let today = options.clock.observed_date().to_string();
    let date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d")?;
    let mut out = Pictures {
        present: true,
        ..Pictures::default()
    };

    let art = WireFetch::new(options.wire())?.for_pictures(SET_ART);
    let r = dumpstar_catalog::art::fetch(
        &options.data_dir.join("art"),
        &art,
        date,
        &dumpstar_catalog::art::sources(),
    )?;
    out.set_art_fetched = r.fetched;
    out.set_art_already = r.already;
    out.failed
        .extend(r.failed.into_iter().map(|f| format!("set art {f}")));

    let sealed = WireFetch::new(options.wire())?.for_pictures(SEALED_IMAGES);
    let r = dumpstar_catalog::images::fetch_pending(
        catalog,
        &sealed,
        &options.data_dir.join("sealed-images"),
        &today,
    )?;
    out.sealed_stored = r.stored;
    out.failed
        .extend(r.failed.into_iter().map(|f| format!("sealed image {f}")));
    Ok(out)
}

fn apply_all(
    catalog: &mut Catalog,
    options: &Options<'_>,
    payloads: &[dumpstar_catalog::RawPayload],
) -> anyhow::Result<Report> {
    let mut report = Report::default();
    if catalog.rejoin_if_stale()? {
        println!("crosswalk: the join changed since this catalog was joined; re-joined");
    }
    for p in payloads {
        let provenance = Provenance {
            source: p.source.clone(),
            url: p.url.clone(),
            fetched_at: options.clock.fetched_at().to_string(),
            sha256: sha256_hex(&p.bytes),
            bytes: p.bytes.len() as u64,
            as_of: p.as_of.clone(),
        };
        if apply(catalog, &p.bytes, &provenance)?.already_applied {
            report.skipped += 1;
        } else {
            report.applied += 1;
        }
    }
    println!(
        "  {} payload(s) applied, {} already applied",
        report.applied, report.skipped
    );
    catalog.record_derivations()?;
    // Format rules are data from the retrieved rules documents, not a payload.
    println!("formats seeded: {} rows changed", catalog.seed_formats()?);
    Ok(report)
}

/// Fetch every upstream a derivation reads, land the bytes, and derive
/// **nothing** — the landing half of [`derive`] on its own.
///
/// This is what `dumpstar data refresh` runs. No catalog is opened, so no row
/// of one is written by this function or by anything it calls.
///
/// The bracket is finalised once, over both halves: the run fails if either
/// did, but both are attempted, because the TCGCSV prices are the half a night
/// cannot lose.
pub fn land(options: &Options<'_>) -> anyhow::Result<Report> {
    let report = Report::default();
    let (official_base, tcgcsv_base) = origins();
    let fetch = WireFetch::new(options.wire())?;

    println!("Landing TCGCSV groups, products, prices...");
    let tcgcsv = (TcgcsvSource {
        http: &fetch,
        base: tcgcsv_base,
        last_updated: None,
    })
    .fetch()
    .inspect(|p| println!("  {} TCGCSV payload(s) landed", p.len()))
    .map_err(anyhow::Error::new);

    println!("Landing the official card pages...");
    let official = (OfficialSource {
        http: &fetch,
        base: official_base,
        since: None,
        crawled_on: None,
        today: options.clock.observed_date().to_string(),
    })
    .fetch()
    .map(|p| println!("  {} official payload(s) landed", p.len()))
    .map_err(anyhow::Error::new);

    if options.landing.is_some() && land_pictures_enabled() {
        land_pictures(
            options,
            tcgcsv.as_ref().map(Vec::as_slice).unwrap_or_default(),
        )?;
    }

    if let Some(landing) = &options.landing {
        // A dead official API must not cost the night's prices, and a landing
        // zone only says so if the official crawl is the ONLY thing marked
        // short. `finalize` marks a dataset incomplete when it recorded a failure
        // of its own, so a failed official fetch does exactly that without an
        // error being passed (which would mark every dataset of the run short).
        // An official error with no failure recorded — a page that fetched and
        // would not parse — leaves a partial prefix that reads as whole, so that
        // one is passed and poisons the night, loudly.
        let recorded = landing.manifests().iter().any(|m| {
            m.source == OFFICIAL.as_str() && m.dataset == CARDS.as_str() && !m.failures.is_empty()
        });
        let official_poisons = official.as_ref().err().filter(|_| !recorded);
        finalize_landing(landing, tcgcsv.as_ref().err().or(official_poisons))?;
    }
    tcgcsv?;
    official?;
    Ok(report)
}

/// Land the pictures the site serves itself: every listed set logo and art
/// crop, and the picture of every sealed product in the `products` payloads
/// just landed. A picture that does not fetch is recorded in its dataset's
/// manifest (which makes that dataset incomplete) and warned about; it never
/// fails the night, because a derive reads an incomplete picture run as far as
/// it goes and `catalog-check` alarms what is still missing.
fn land_pictures(
    options: &Options<'_>,
    tcgcsv: &[dumpstar_catalog::RawPayload],
) -> anyhow::Result<()> {
    let mut sealed = Vec::new();
    for p in tcgcsv {
        let path = p.url.split(['?', '#']).next().unwrap_or(&p.url);
        if path.rsplit('/').next() == Some("products") {
            sealed.extend(dumpstar_catalog::images::urls_of_products(&p.bytes)?);
        }
    }
    let set_art = dumpstar_catalog::art::urls(&dumpstar_catalog::art::sources());
    for (dataset, urls) in [(SET_ART, set_art), (SEALED_IMAGES, sealed)] {
        println!("Landing {} {dataset} picture(s)...", urls.len());
        let fetch = WireFetch::new(options.wire())?.for_pictures(dataset);
        let mut failed = 0;
        for (label, url) in urls {
            match fetch.get(&url) {
                Ok(b) if !b.is_empty() => {}
                Ok(_) => {
                    failed += 1;
                    eprintln!("WARN: {dataset} {label}: empty body");
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("WARN: {dataset} {label}: {e}");
                }
            }
        }
        if failed > 0 {
            println!("  {failed} {dataset} picture(s) did not land");
        }
    }
    Ok(())
}
