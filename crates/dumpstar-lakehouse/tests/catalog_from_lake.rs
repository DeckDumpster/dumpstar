//! The SWU catalog derived from the lake's `raw/` is the catalog the legacy
//! ONLINE apply (`dumpstar catalog-apply --swu`) builds — row for row, cards
//! with their mechanics, and prices (sp-6i11rn).
//!
//! Both sides read the recorded SOR load in `fixtures/swu/` through one fixture
//! upstream on loopback:
//!
//! | | |
//! | --- | --- |
//! | reference | the online path, as `catalog-apply --swu` ran it: `OfficialSource` and `TcgcsvSource` over `HttpFetch`, landed on local disk, applied |
//! | candidate | `dumpstar_derive::land` into a lake (`DirStore`), then the shipped `dumpstar-lake-derive shared`, which replays it |
//!
//! The comparison is `dumpstar-lake-derive diff`, excluding only
//! `raw_derivation` (the offline job's provenance, which the online path never
//! wrote).

#[allow(dead_code)]
#[path = "../../dumpstar-ingest/tests/support/mod.rs"]
mod support;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dumpstar_catalog::Catalog;
use dumpstar_catalog::fetch::HttpFetch;
use dumpstar_catalog::job::run_once;
use dumpstar_catalog::swu::{OfficialSource, TcgcsvSource};
use dumpstar_derive::DeriveClock;
use dumpster_kit_lake::{DirStore, RawLanding};
use support::{FakeUpstream, Reply};

const DATE: &str = "2026-10-12";
/// The landing run's clock. The stamp the fixture upstream serves is
/// `2026-10-12T20:05:44+0000`, the day its prices describe.
const CLOCK: &str = "2026-10-13T06:01:00+00:00";

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/swu")
}

fn read(rel: &str) -> Vec<u8> {
    let bytes = std::fs::read(fixtures().join(rel)).unwrap();
    if rel.ends_with(".gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut out)
            .unwrap();
        out
    } else {
        bytes
    }
}

/// The recorded SOR load, served by URL. `official` false makes the official
/// API answer 503, which is how a night with a dead official API is produced.
fn start_upstream(official: bool) -> FakeUpstream {
    let mut pages: Vec<Vec<u8>> = (1..=10)
        .map(|i| read(&format!("official-sor-pages/sor-page{i}.json.gz")))
        .collect();
    // The recorded pages carry no rules text, so no card has a mechanic. One
    // synthetic entry that does rides the last page: a Force consumer and a
    // Shield producer, which is what makes the mechanics comparison mean something.
    let mut last: serde_json::Value = serde_json::from_slice(&pages[9]).unwrap();
    last["data"].as_array_mut().unwrap().push(serde_json::json!({
        "id": 99001,
        "attributes": {
            "title": "Lakebed Sentinel", "subtitle": null, "cardUid": "9900100001",
            "cardNumber": 901, "serialCode": "0101901", "cardId": null,
            "text": "When Played: You may use the Force. If you do, give a Shield token to this unit.",
            "expansion": {"data": {"attributes": {"code": "SOR", "name": "Spark of Rebellion", "sortValue": 1}}},
            "variantTypes": {"data": [{"attributes": {"name": "Standard", "foil": false}}]},
            "rarity": {"data": {"attributes": {"name": "Common"}}},
            "type": {"data": {"attributes": {"name": "Unit"}}},
            "keywords": {"data": []}
        }
    }));
    pages[9] = serde_json::to_vec(&last).unwrap();
    let groups = read("swu-groups.json");
    let stamp = String::from_utf8(read("last-updated.txt")).unwrap();
    let mut products = std::collections::BTreeMap::new();
    let mut prices = std::collections::BTreeMap::new();
    products.insert(23405, read("sor-products.json.gz"));
    prices.insert(23405, read("sor-prices.json"));
    products.insert(23451, read("sor-wpp-products.json"));
    prices.insert(23451, read("sor-wpp-prices.json"));
    for g in [23452, 23453, 23454, 23455] {
        products.insert(g, read(&format!("promo-{g}-products.json.gz")));
        prices.insert(g, read(&format!("promo-{g}-prices.json.gz")));
    }
    FakeUpstream::start(move |target, _n| {
        let decoded = target.replace("%5B", "[").replace("%5D", "]");
        // The publishers' image hosts, folded onto this one by
        // DUMPSTAR_PICTURE_BASE_URL: every picture is bytes that name its path,
        // so a picture stored under the wrong name is a different file.
        let bare = decoded.split(['?', '#']).next().unwrap_or(&decoded);
        if ["png", "jpg", "jpeg", "webp"]
            .iter()
            .any(|e| bare.ends_with(&format!(".{e}")))
        {
            return Reply::png(format!("picture {bare}").into_bytes());
        }
        if decoded.starts_with("/api/cards") {
            if !official {
                return Reply::status(503, r#"{"error":"official is down"}"#);
            }
            let n: usize = decoded
                .split("pagination[page]=")
                .nth(1)
                .and_then(|r| r.split('&').next())
                .and_then(|n| n.parse().ok())
                .expect("a page number");
            return Reply::ok(String::from_utf8(pages[n - 1].clone()).unwrap());
        }
        if decoded.starts_with("/last-updated.txt") {
            return Reply::ok(stamp.clone());
        }
        let parts: Vec<&str> = decoded.trim_matches('/').split('/').collect();
        match parts.as_slice() {
            ["tcgplayer", _, "groups"] => Reply::ok(String::from_utf8(groups.clone()).unwrap()),
            ["tcgplayer", _, g, leaf] => {
                let g: i64 = g.parse().unwrap();
                let map = if *leaf == "products" {
                    &products
                } else {
                    &prices
                };
                Reply::ok(
                    map.get(&g)
                        .map(|b| String::from_utf8(b.clone()).unwrap())
                        .unwrap_or_else(|| r#"{"success":true,"results":[]}"#.to_string()),
                )
            }
            _ => Reply::status(404, format!("no route for {target}")),
        }
    })
}

struct Harness {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    upstream: FakeUpstream,
}

impl Harness {
    fn start(official: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        Self {
            root: tmp.path().to_path_buf(),
            _tmp: tmp,
            upstream: start_upstream(official),
        }
    }

    fn raw(&self) -> PathBuf {
        self.root.join("raw-zone")
    }

    fn db(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.sqlite"))
    }

    fn point_env_at_upstream(&self) -> std::sync::MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: every writer of these variables in this binary holds the lock.
        unsafe {
            std::env::set_var("DUMPSTAR_TCGCSV_BASE_URL", self.upstream.base_url());
            std::env::set_var("DUMPSTAR_OFFICIAL_BASE_URL", self.upstream.base_url());
            std::env::set_var("DUMPSTAR_PICTURE_BASE_URL", self.upstream.base_url());
        }
        guard
    }

    /// What `catalog-apply --swu` ran, minus the pictures: the reference.
    fn legacy_online(&self, db: &Path) {
        // The open converges the catalog; the apply job then opens its own
        // connection, exactly as the CLI did.
        drop(dumpstar_db::open_shared(db).unwrap());
        let raw = self.root.join("legacy-raw");
        let http = HttpFetch::new(Duration::ZERO).unwrap();
        Catalog::open_writer(db).unwrap().rejoin_if_stale().unwrap();
        // The CLI re-applied what an older derivation had landed before it
        // fetched; on a first night that is nothing, and it records the
        // derivation versions as in force.
        assert_eq!(
            Catalog::open_writer(db)
                .unwrap()
                .reapply_landed(&raw)
                .unwrap(),
            0
        );
        let official = OfficialSource {
            http: &http,
            base: self.upstream.base_url(),
            since: None,
            crawled_on: None,
            today: "2026-10-13".into(),
        };
        run_once(&official, &raw, db, chrono::Utc::now()).unwrap();
        let tcgcsv = TcgcsvSource {
            http: &http,
            base: self.upstream.base_url(),
            last_updated: None,
        };
        run_once(&tcgcsv, &raw, db, chrono::Utc::now()).unwrap();
        Catalog::open_writer(db).unwrap().seed_formats().unwrap();
        // The next open converges what the payloads gave it to fold, as every
        // serving start does.
        let mut conn = dumpstar_db::open_shared(db).unwrap();
        dumpstar_db::converge_shared(&mut conn).unwrap();
    }

    /// What `catalog-apply --assets` ran: the reference for the pictures. The
    /// publishers' hosts are folded onto the fixture upstream the way
    /// `DUMPSTAR_PICTURE_BASE_URL` folds them for the lake's landing.
    fn legacy_pictures(&self, db: &Path, into: &Path) {
        let fold = Folded {
            http: HttpFetch::new(Duration::ZERO).unwrap(),
            base: self.upstream.base_url(),
        };
        dumpstar_catalog::art::fetch(
            &into.join("art"),
            &fold,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
            &dumpstar_catalog::art::sources(),
        )
        .unwrap();
        dumpstar_catalog::images::fetch_pending(
            &Catalog::open_writer(db).unwrap(),
            &fold,
            &into.join("sealed-images"),
            "2026-10-13",
        )
        .unwrap();
    }

    /// The landing unit: `dumpstar data refresh`.
    fn land(&self) -> anyhow::Result<()> {
        let _guard = self.point_env_at_upstream();
        let clock = DeriveClock::from_manifest(CLOCK, "the test's clock").unwrap();
        let landing = Arc::new(RawLanding::new(
            Box::new(DirStore::new(self.raw())),
            DATE,
            clock.fetched_at(),
        ));
        dumpstar_derive::land(&dumpstar_derive::Options {
            clock,
            data_dir: &self.root,
            landing: Some(landing),
            replay: None,
        })
        .map(|_| ())
    }

    fn derive(&self, db: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dumpstar-lake-derive"))
            .args(["shared", "--ingest-date", DATE, "--db"])
            .arg(db)
            .arg("--data-dir")
            .arg(&self.root)
            .env("DUMPSTAR_LAKE_DIR", self.raw())
            .env("DUMPSTAR_TCGCSV_BASE_URL", self.upstream.base_url())
            .env("DUMPSTAR_OFFICIAL_BASE_URL", self.upstream.base_url())
            .env("DUMPSTAR_PICTURE_BASE_URL", self.upstream.base_url())
            .env("DUMPSTAR_LAKE_ENV", self.root.join("no-such-lake.env"))
            .output()
            .unwrap()
    }

    fn diff(&self, left: &Path, right: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dumpstar-lake-derive"))
            .args([
                "diff",
                "--max-diffs",
                "50",
                "--exclude",
                "raw_derivation",
                "--left",
            ])
            .arg(left)
            .arg("--right")
            .arg(right)
            .output()
            .unwrap()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn scalar<T: rusqlite::types::FromSql>(db: &Path, sql: &str) -> T {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

/// **The acceptance criterion.** The catalog built from the lake is the one
/// today's online apply builds: its cards (mechanics included), its printings
/// and its prices.
#[test]
fn the_catalog_derived_from_the_lake_is_row_identical_to_the_online_apply() {
    let h = Harness::start(true);
    let online = h.db("online");
    let offline = h.db("offline");

    h.legacy_online(&online);
    h.legacy_pictures(&online, &h.root.join("reference"));
    h.land().expect("the landing run");
    let out = h.derive(&offline);
    assert!(out.status.success(), "derive failed:\n{}", text(&out));
    assert!(
        text(&out).contains("raw coverage: complete"),
        "{}",
        text(&out)
    );

    // Not a vacuous comparison: there are cards, printings and prices to match.
    assert!(scalar::<i64>(&offline, "SELECT COUNT(*) FROM printings") > 500);
    assert!(scalar::<i64>(&offline, "SELECT COUNT(*) FROM cards") > 100);
    assert!(
        scalar::<i64>(
            &offline,
            "SELECT COUNT(*) FROM cards WHERE mechanics != '[]'"
        ) > 0
    );
    assert!(scalar::<i64>(&offline, "SELECT COUNT(*) FROM prices") > 500);
    assert!(scalar::<i64>(&offline, "SELECT COUNT(*) FROM sealed_prices") > 0);

    let diff = h.diff(&online, &offline);
    assert!(
        diff.status.success(),
        "the lake's catalog is not row-identical to the online one:\n{}",
        text(&diff)
    );
    assert!(text(&diff).contains("ROW-IDENTICAL"));
}

/// The derive reaches no upstream: the fixture upstream sees no request from it.
#[test]
fn the_derive_never_asks_an_upstream() {
    let h = Harness::start(true);
    h.land().expect("the landing run");
    let landed = h.upstream.requests().len();
    let out = h.derive(&h.db("offline"));
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        h.upstream.requests().len(),
        landed,
        "the derive sent a request to the upstream"
    );
}

/// A night with a dead official API still lands and derives the prices, and the
/// derive says the card list is stale by exiting 2.
#[test]
fn a_dead_official_api_still_derives_the_prices_and_exits_2() {
    let h = Harness::start(false);
    assert!(
        h.land().is_err(),
        "the landing run must fail loudly when the official API is down"
    );
    let db = h.db("offline");
    let out = h.derive(&db);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("TCGCSV half alone"), "{}", text(&out));
    assert!(scalar::<i64>(&db, "SELECT COUNT(*) FROM tcgcsv_products") > 0);
    assert_eq!(scalar::<i64>(&db, "SELECT COUNT(*) FROM cards"), 0);
}

/// A derivation-version bump re-derives everything from raw: forget one
/// payload as applied under the current version, derive the same date again,
/// and it is applied again.
#[test]
fn a_derivation_bump_rederives_from_raw() {
    let h = Harness::start(true);
    h.land().expect("the landing run");
    let db = h.db("offline");
    assert!(h.derive(&db).status.success());
    let cards: i64 = scalar(&db, "SELECT COUNT(*) FROM cards");

    // What a bump does to a stored catalog: its payloads are no longer applied
    // under the key the new build computes.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("DELETE FROM applied_payloads WHERE source = 'official'", [])
            .unwrap();
        conn.execute("UPDATE cards SET mechanics = '[]'", [])
            .unwrap();
    }
    assert_eq!(
        scalar::<i64>(&db, "SELECT COUNT(*) FROM cards WHERE mechanics != '[]'"),
        0
    );
    let out = h.derive(&db);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(scalar::<i64>(&db, "SELECT COUNT(*) FROM cards"), cards);
    assert!(
        scalar::<i64>(&db, "SELECT COUNT(*) FROM cards WHERE mechanics != '[]'") > 0,
        "the cards were not re-derived from raw"
    );
}

/// A fetch that asks the fixture upstream for a publisher's URL path.
struct Folded {
    http: HttpFetch,
    base: String,
}

impl dumpstar_catalog::fetch::Fetch for Folded {
    fn get(&self, url: &str) -> dumpstar_catalog::Result<Vec<u8>> {
        let (_, rest) = url.split_once("://").unwrap();
        let path = &rest[rest.find('/').unwrap()..];
        self.http.get(&format!("{}{path}", self.base))
    }
}

/// Every file under `dir` by relative path, with its bytes.
fn files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_file() && name != "manifest.json" {
            out.insert(name, std::fs::read(e.path()).unwrap());
        }
    }
    out
}

/// **sp-m1fuer's acceptance.** The set art and sealed pictures the derive
/// replays from the lake are the ones the online `--assets` job fetched: the
/// same files, byte for byte, and the derive asked no upstream for them.
#[test]
fn the_pictures_replayed_from_the_lake_are_those_the_online_job_fetched() {
    let h = Harness::start(true);
    let online = h.db("online");
    let offline = h.db("offline");

    h.legacy_online(&online);
    let reference = h.root.join("reference");
    h.legacy_pictures(&online, &reference);
    let want_art = files(&reference.join("art"));
    let want_sealed = files(&reference.join("sealed-images"));
    assert!(want_art.len() > 30, "{} set art files", want_art.len());
    assert!(!want_sealed.is_empty(), "no sealed pictures to compare");

    h.land().expect("the landing run");
    let landed = h.upstream.requests().len();
    let out = h.derive(&offline);
    assert!(out.status.success(), "derive failed:\n{}", text(&out));
    assert_eq!(
        h.upstream.requests().len(),
        landed,
        "the derive sent a request to the upstream"
    );

    assert_eq!(files(&h.root.join("art")), want_art, "set art");
    assert_eq!(
        files(&h.root.join("sealed-images")),
        want_sealed,
        "sealed pictures"
    );
    assert_eq!(
        scalar::<i64>(
            &offline,
            "SELECT COUNT(*) FROM sealed_images WHERE retrieved_on IS NULL"
        ),
        0
    );
}

/// A night with no pictures landed still derives the catalog, and says so.
#[test]
fn a_partition_without_pictures_still_derives_the_catalog() {
    let h = Harness::start(true);
    h.land().expect("the landing run");
    let assets = h.raw().join("raw/source=swu-assets");
    assert!(assets.is_dir(), "the landing run landed no pictures");
    std::fs::remove_dir_all(assets).unwrap();
    let db = h.db("offline");
    let out = h.derive(&db);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no pictures landed"), "{}", text(&out));
    assert!(scalar::<i64>(&db, "SELECT COUNT(*) FROM printings") > 500);
}
