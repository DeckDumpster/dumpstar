//! Acceptance for the official-API and tcgcsv adapters (design row S7), built on
//! the recorded payloads in `fixtures/swu/`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use dumpstar_catalog::fetch::Fetch;
use dumpstar_catalog::job::{JobReport, run_once};
use dumpstar_catalog::swu::{OfficialSource, TcgcsvSource};
use dumpstar_catalog::{Catalog, check, official};
use rusqlite::Connection;
use serde_json::{Value, json};

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

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

/// Serves recorded bytes by URL and remembers what it was asked for.
struct Recorded {
    official_pages: Vec<Vec<u8>>,
    last_updated: String,
    groups: Vec<u8>,
    products: BTreeMap<i64, Vec<u8>>,
    prices: BTreeMap<i64, Vec<u8>>,
    asked: RefCell<Vec<String>>,
}

impl Fetch for Recorded {
    fn get(&self, url: &str) -> dumpstar_catalog::Result<Vec<u8>> {
        self.asked.borrow_mut().push(url.to_string());
        if let Some(rest) = url.split("pagination[page]=").nth(1) {
            let n: usize = rest.split('&').next().unwrap().parse().unwrap();
            return Ok(self.official_pages[n - 1].clone());
        }
        if url.ends_with("last-updated.txt") {
            return Ok(self.last_updated.clone().into_bytes());
        }
        if url.ends_with("/groups") {
            return Ok(self.groups.clone());
        }
        let parts: Vec<&str> = url.rsplit('/').take(2).collect();
        let gid: i64 = parts[1].parse().unwrap();
        let map = if parts[0] == "products" {
            &self.products
        } else {
            &self.prices
        };
        Ok(map
            .get(&gid)
            .cloned()
            .unwrap_or_else(|| br#"{"success":true,"results":[]}"#.to_vec()))
    }
}

fn sor() -> Recorded {
    let mut products = BTreeMap::new();
    let mut prices = BTreeMap::new();
    products.insert(23405, read("sor-products.json.gz"));
    prices.insert(23405, read("sor-prices.json"));
    products.insert(23451, read("sor-wpp-products.json"));
    prices.insert(23451, read("sor-wpp-prices.json"));
    for g in [23452, 23453, 23454, 23455] {
        products.insert(g, read(&format!("promo-{g}-products.json.gz")));
        prices.insert(g, read(&format!("promo-{g}-prices.json.gz")));
    }
    Recorded {
        official_pages: (1..=10)
            .map(|i| read(&format!("official-sor-pages/sor-page{i}.json.gz")))
            .collect(),
        last_updated: String::from_utf8(read("last-updated.txt")).unwrap(),
        groups: read("swu-groups.json"),
        products,
        prices,
        asked: RefCell::new(vec![]),
    }
}

struct Env {
    _dir: tempfile::TempDir,
    raw: PathBuf,
    db: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    Env {
        raw: dir.path().join("raw"),
        db: dir.path().join("shared.sqlite"),
        _dir: dir,
    }
}

fn state(e: &Env, key: &str) -> Option<String> {
    Catalog::open_writer(&e.db)
        .unwrap()
        .sync_state(key)
        .unwrap()
}

fn day_of(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%d").to_string()
}

fn sync_official(e: &Env, http: &dyn Fetch, now: DateTime<Utc>) -> Vec<JobReport> {
    let src = OfficialSource {
        http,
        base: "https://official.test".into(),
        since: state(e, "official.updated_since"),
        crawled_on: state(e, "official.crawled_on"),
        today: day_of(now),
    };
    run_once(&src, &e.raw, &e.db, now).unwrap()
}

fn sync_tcgcsv(e: &Env, http: &dyn Fetch, now: DateTime<Utc>) -> Vec<JobReport> {
    let src = TcgcsvSource {
        http,
        base: "https://tcgcsv.test".into(),
        last_updated: state(e, "tcgcsv.last_updated"),
    };
    run_once(&src, &e.raw, &e.db, now).unwrap()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn alarms(conn: &Connection, kind: &str) -> Vec<String> {
    let mut s = conn
        .prepare("SELECT key FROM alarms WHERE kind = ?1 ORDER BY key")
        .unwrap();
    s.query_map([kind], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn uid_of(conn: &Connection, set: &str, number: &str, variant: &str, finish: &str) -> String {
    conn.query_row(
        "SELECT printing_id FROM printings WHERE set_code=?1 AND number=?2 AND variant=?3 AND finish=?4",
        [set, number, variant, finish],
        |r| r.get(0),
    )
    .unwrap()
}

fn priced(conn: &Connection, printing: &str, day: &str) -> bool {
    count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM prices WHERE printing_id='{printing}' AND source='tcgcsv' AND observed_on='{day}'"
        ),
    ) == 1
}

fn all_ids(conn: &Connection) -> BTreeSet<String> {
    let mut s = conn.prepare("SELECT printing_id FROM printings").unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

// ---- parsers on the recorded payloads -------------------------------------

#[test]
fn the_single_card_fetch_parses_to_one_hyperspace_printing() {
    let page = official::parse_page(&read("off-single-uid.json")).unwrap();
    let p = &page.payload.printings[0];
    assert_eq!(p.printing_id, "6202608327");
    assert_eq!((p.set_code.as_str(), p.number.as_str()), ("SOR", "1"));
    assert_eq!(
        (p.variant.as_str(), p.finish.as_str()),
        ("Hyperspace", "Normal")
    );
    assert_eq!(p.official_card_uid.as_deref(), Some("6202608327"));
    let c = &page.payload.cards[0];
    assert_eq!(c.card_id, "2579145458");
    assert_eq!(
        (c.name.as_str(), c.subtitle.as_deref()),
        ("Luke Skywalker", Some("Faithful Friend"))
    );
    assert_eq!((c.cost, c.power, c.hp), (Some(6), Some(4), Some(7)));
    assert!(c.rules_text.as_deref().unwrap().contains("Shield token"));
    assert!(page.max_updated_at.is_some());
}

#[test]
fn the_bulk_page_parses_both_entries_and_reports_the_page_count() {
    let page = official::parse_page(&read("off-cards-p2.json.gz")).unwrap();
    assert_eq!(page.payload.printings.len(), 2);
    assert_eq!(page.page_count, 5097);
    assert!(page.payload.alarms.is_empty());
}

#[test]
fn a_malformed_official_entry_is_an_alarm_not_a_dropped_row() {
    let bytes = serde_json::to_vec(&json!({"data": [{"id": 7, "attributes": {"title": "x"}}],
        "meta": {"pagination": {"pageCount": 1}}}))
    .unwrap();
    let page = official::parse_page(&bytes).unwrap();
    assert!(page.payload.printings.is_empty());
    assert_eq!(page.payload.alarms[0].kind, "official_bad_record");
}

// ---- the load --------------------------------------------------------------

#[test]
fn a_full_sor_load_has_every_official_printing_and_keeps_ids_across_a_second_load() {
    let e = env();
    let http = sor();
    sync_official(&e, &http, at(13, 4));
    let conn = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(count(conn.conn(), "SELECT COUNT(*) FROM printings"), 991);
    let before = all_ids(conn.conn());
    drop(conn);

    // The same catalog, re-serialised (a new hash), on a later day, and one
    // card's rarity changed upstream.
    let mut pages: Vec<Vec<u8>> = http
        .official_pages
        .iter()
        .map(|p| serde_json::to_vec_pretty(&serde_json::from_slice::<Value>(p).unwrap()).unwrap())
        .collect();
    let mut first: Value = serde_json::from_slice(&pages[0]).unwrap();
    first["data"][0]["attributes"]["rarity"]["data"]["attributes"]["name"] = json!("Mythic");
    pages[0] = serde_json::to_vec_pretty(&first).unwrap();
    let again = Recorded {
        official_pages: pages,
        ..sor()
    };
    let r = sync_official(&e, &again, at(14, 4));
    assert!(
        r.iter().any(|j| j.applied.printings == 1),
        "only the edited printing is written"
    );
    let conn = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(
        all_ids(conn.conn()),
        before,
        "every printing keeps its printing_id"
    );
    assert_eq!(count(conn.conn(), "SELECT COUNT(*) FROM printings"), 991);
}

/// A 102-page crawl of 10,193 entries, as the real API serves it, built from the
/// recorded SOR entries so the shape is real and the count is the acceptance's.
fn synthetic_pages(total: usize) -> Vec<Vec<u8>> {
    let sor: Vec<Value> = (1..=10)
        .flat_map(|i| {
            let v: Value =
                serde_json::from_slice(&read(&format!("official-sor-pages/sor-page{i}.json.gz")))
                    .unwrap();
            v["data"].as_array().unwrap().clone()
        })
        .collect();
    let slim: Vec<Value> = sor
        .iter()
        .map(|e| {
            let a = &e["attributes"];
            let mut o = json!({});
            for k in [
                "title",
                "subtitle",
                "cardNumber",
                "serialCode",
                "hyperspace",
                "showcase",
                "cardId",
                "expansion",
                "variantTypes",
                "rarity",
            ] {
                o[k] = a[k].clone();
            }
            o
        })
        .collect();
    let pages = total.div_ceil(100);
    (0..pages)
        .map(|p| {
            let data: Vec<Value> = (p * 100..((p + 1) * 100).min(total))
                .map(|i| {
                    let mut a = slim[i % slim.len()].clone();
                    let block = i / slim.len();
                    let serial = a["serialCode"].as_str().unwrap().to_string();
                    a["serialCode"] = json!(format!("{:02}{}", block, &serial[2..]));
                    a["cardUid"] = json!(format!("{}", 1_000_000_000 + i));
                    a["updatedAt"] = json!(format!("2026-09-{:02}T10:00:00.000Z", 1 + i % 28));
                    a["expansion"]["data"]["attributes"]["code"] = json!(format!("X{block:02}"));
                    json!({"id": i + 1, "attributes": a})
                })
                .collect();
            serde_json::to_vec(&json!({"data": data, "meta": {"pagination": {"page": p + 1, "pageSize": 100, "pageCount": pages, "total": total}}})).unwrap()
        })
        .collect()
}

#[test]
fn a_full_crawl_is_10193_printings_and_every_one_keeps_its_id_on_a_second_load() {
    let e = env();
    let mut http = sor();
    http.official_pages = synthetic_pages(10_193);
    assert_eq!(http.official_pages.len(), 102);
    sync_official(&e, &http, at(13, 4));
    let before = {
        let c = Catalog::open_readonly(&e.db).unwrap();
        assert_eq!(count(c.conn(), "SELECT COUNT(*) FROM printings"), 10_193);
        all_ids(c.conn())
    };
    assert_eq!(before.len(), 10_193);
    assert_eq!(http.asked.borrow().len(), 102);

    // Next day: a delta crawl resumes from the largest updatedAt it saw, and a
    // full re-serialised crawl changes nothing.
    http.asked.borrow_mut().clear();
    http.official_pages = http
        .official_pages
        .iter()
        .map(|p| serde_json::to_vec_pretty(&serde_json::from_slice::<Value>(p).unwrap()).unwrap())
        .collect();
    let second = sync_official(&e, &http, at(14, 4));
    let asked = http.asked.borrow();
    assert!(
        asked[0].contains("filters[updatedAt][$gte]=2026-09-28T10:00:00.000Z"),
        "delta handle missing: {}",
        asked[0]
    );
    for j in &second {
        let a = &j.applied;
        assert_eq!(
            a.sets + a.cards + a.printings + a.crosswalk + a.alarms,
            0,
            "{a:?}"
        );
    }
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(all_ids(c.conn()), before);
}

#[test]
fn at_most_one_official_crawl_a_day() {
    let e = env();
    let http = sor();
    sync_official(&e, &http, at(13, 4));
    let n = http.asked.borrow().len();
    assert_eq!(n, 10);
    let r = sync_official(&e, &http, at(13, 16));
    assert!(r.is_empty());
    assert_eq!(http.asked.borrow().len(), n, "no request on the same day");
}

#[test]
fn the_official_urls_are_the_prescribed_query() {
    let http = sor();
    let src = OfficialSource {
        http: &http,
        base: "https://admin.starwarsunlimited.com".into(),
        since: None,
        crawled_on: None,
        today: "2026-10-13".into(),
    };
    let u = src.page_url(3);
    for want in [
        "/api/cards?",
        "locale=en",
        "pagination[pageSize]=100",
        "pagination[page]=3",
        "sort[0]=id:asc",
        "populate[0]=expansion",
        "populate[1]=variantTypes",
        "populate[2]=rarity",
    ] {
        assert!(u.contains(want), "{want} missing from {u}");
    }
    assert!(dumpstar_catalog::fetch::USER_AGENT.contains("dumpstar"));
}

// ---- prices and the join ---------------------------------------------------

fn loaded() -> (Env, Recorded) {
    let e = env();
    let http = sor();
    sync_official(&e, &http, at(13, 4));
    sync_tcgcsv(&e, &http, at(13, 4));
    (e, http)
}

#[test]
fn at_least_99_percent_of_non_token_printings_get_a_price_row() {
    let (e, _) = loaded();
    let c = Catalog::open_readonly(&e.db).unwrap();
    let non_token = "p.number NOT GLOB '[Tt][0-9]*'";
    let total = count(
        c.conn(),
        &format!("SELECT COUNT(*) FROM printings p WHERE {non_token}"),
    );
    let with = count(
        c.conn(),
        &format!(
            "SELECT COUNT(*) FROM printings p WHERE {non_token} AND EXISTS
               (SELECT 1 FROM prices x WHERE x.printing_id = p.printing_id AND x.observed_on = '2026-10-12')"
        ),
    );
    assert_eq!(total, 987);
    assert!(
        with as f64 / total as f64 >= 0.99,
        "{with} of {total} printings priced"
    );
    let cov = check::coverage(c.conn(), "2026-10-12").unwrap();
    assert!(cov.ratio() >= 0.99, "{cov:?}");
}

#[test]
fn official_identity_wins_and_tcgcsv_only_supplies_ids_and_prices() {
    let (e, _) = loaded();
    let c = Catalog::open_readonly(&e.db).unwrap();
    // Takedown: official number 77; tcgcsv Number 077/252, one product for both finishes.
    let normal = uid_of(c.conn(), "SOR", "77", "Standard", "Normal");
    let foil = uid_of(c.conn(), "SOR", "77", "Standard", "Foil");
    assert_ne!(normal, foil);
    let pid = |p: &str, sub: &str| -> Option<String> {
        c.conn()
            .query_row(
                "SELECT source_id FROM printing_source_id WHERE source='tcgcsv' AND printing_id=?1 AND sub_type_name=?2",
                [p, sub],
                |r| r.get(0),
            )
            .ok()
    };
    assert_eq!(pid(&normal, "Normal").as_deref(), Some("540198"));
    assert_eq!(pid(&foil, "Foil").as_deref(), Some("540198"));
    assert!(priced(c.conn(), &normal, "2026-10-12") && priced(c.conn(), &foil, "2026-10-12"));
    // The Hyperspace Luke is listed by tcgcsv in the Event Exclusive Promos group.
    let hs = uid_of(c.conn(), "SOR", "1", "Hyperspace", "Normal");
    assert_eq!(pid(&hs, "Normal").as_deref(), Some("542170"));
    assert!(priced(c.conn(), &hs, "2026-10-12"));
    // Weekly Play promos join through the hand-kept group table.
    let wp = uid_of(c.conn(), "SOR", "20", "Weekly Play", "Normal");
    assert!(priced(c.conn(), &wp, "2026-10-12"));
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM cards WHERE name = 'Luke Skywalker'"
        ),
        2
    );
}

#[test]
fn token_backs_are_handled_not_counted_as_misses() {
    let (e, _) = loaded();
    let c = Catalog::open_readonly(&e.db).unwrap();
    // 'Capital City // Shield' (20 // T02) and the like price nothing and alarm nothing.
    let alarmed: Vec<String> = [
        "official_without_tcgcsv",
        "tcgcsv_without_official",
        "unmatched_price",
    ]
    .iter()
    .flat_map(|k| alarms(c.conn(), k))
    .collect();
    for pid in 542201..=542232 {
        assert!(
            !alarmed.contains(&pid.to_string()),
            "token-back product {pid} alarmed"
        );
    }
    assert!(alarmed.iter().all(|k| !k.starts_with("542")), "{alarmed:?}");
    // The standalone token product prices the first token, and the second token
    // of the pair is a counted token back, not an alarm.
    let exp = uid_of(c.conn(), "SOR", "T01", "Standard", "Normal");
    assert!(priced(c.conn(), &exp, "2026-10-12"));
    let shield = uid_of(c.conn(), "SOR", "T02", "Standard", "Normal");
    assert!(!alarmed.contains(&shield));
    assert!(
        alarms(c.conn(), "official_unlisted")
            .iter()
            .all(|k| *k != shield)
    );
    assert_eq!(
        count(
            c.conn(),
            &format!("SELECT COUNT(*) FROM prices WHERE printing_id='{shield}'")
        ),
        0
    );
}

#[test]
fn the_recorded_sor_load_raises_no_alarm_of_its_own() {
    let (e, _) = loaded();
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert!(
        check::stored(c.conn()).unwrap().is_empty(),
        "{:?}",
        check::stored(c.conn()).unwrap()
    );
    let info = count(
        c.conn(),
        "SELECT COUNT(*) FROM alarms WHERE severity='info'",
    );
    assert!(info > 0, "unlisted promos are counted, not hidden");
}

#[test]
fn sealed_products_get_their_own_price_series_and_the_card_prices_are_unchanged() {
    let (e, _) = loaded();
    let c = Catalog::open_readonly(&e.db).unwrap();
    let sealed = count(
        c.conn(),
        "SELECT COUNT(*) FROM tcgcsv_products WHERE number IS NULL",
    );
    assert!(sealed > 0, "the recorded load has sealed products");
    let stored = count(c.conn(), "SELECT COUNT(*) FROM sealed_prices");
    assert!(stored > 0, "sealed price rows are stored");
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM sealed_prices s
              WHERE NOT EXISTS (SELECT 1 FROM tcgcsv_products t
                                 WHERE t.product_id = s.tcgplayer_product_id AND t.number IS NULL)"
        ),
        0,
        "only numberless products land in the sealed series"
    );
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM prices WHERE observed_on = '2026-10-12'"
        ),
        count(
            c.conn(),
            "SELECT COUNT(*) FROM prices p JOIN printings g USING (printing_id)
              WHERE p.observed_on = '2026-10-12'"
        ),
        "the card series holds card printings only"
    );
    let again = {
        let http = sor();
        sync_tcgcsv(&e, &http, at(13, 5));
        count(c.conn(), "SELECT COUNT(*) FROM sealed_prices")
    };
    assert_eq!(again, stored, "a second sync of the same day adds nothing");
}

fn edit_products(http: &mut Recorded, f: impl FnOnce(&mut Vec<Value>)) {
    let mut v: Value = serde_json::from_slice(&http.products[&23405]).unwrap();
    f(v["results"].as_array_mut().unwrap());
    http.products.insert(23405, serde_json::to_vec(&v).unwrap());
}

#[test]
fn a_printing_missing_from_tcgcsv_raises_an_alarm() {
    let e = env();
    let mut http = sor();
    edit_products(&mut http, |r| r.retain(|p| p["productId"] != 540198));
    sync_official(&e, &http, at(13, 4));
    sync_tcgcsv(&e, &http, at(13, 4));
    let c = Catalog::open_readonly(&e.db).unwrap();
    let normal = uid_of(c.conn(), "SOR", "77", "Standard", "Normal");
    let foil = uid_of(c.conn(), "SOR", "77", "Standard", "Foil");
    let got = alarms(c.conn(), "official_without_tcgcsv");
    assert!(got.contains(&normal) && got.contains(&foil), "{got:?}");
    assert!(!priced(c.conn(), &normal, "2026-10-12"));
    // The orphaned price rows are an alarm too, not a silent drop.
    assert!(alarms(c.conn(), "unmatched_price").contains(&"540198:Normal".to_string()));
    assert!(!check::stored(c.conn()).unwrap().is_empty());
}

#[test]
fn a_tcgcsv_product_with_no_official_printing_raises_an_alarm_and_is_never_merged() {
    let e = env();
    let mut http = sor();
    edit_products(&mut http, |r| {
        let mut ghost = r.iter().find(|p| p["productId"] == 540198).unwrap().clone();
        ghost["productId"] = json!(999_001);
        ghost["name"] = json!("Ghost Trooper");
        ghost["extendedData"] = json!([{"name": "Number", "value": "999/252"}]);
        r.push(ghost);
    });
    sync_official(&e, &http, at(13, 4));
    sync_tcgcsv(&e, &http, at(13, 4));
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(
        alarms(c.conn(), "tcgcsv_without_official"),
        vec!["999001".to_string()]
    );
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM printing_source_id WHERE source_id='999001'"
        ),
        0
    );
}

#[test]
fn a_number_that_points_at_another_card_is_not_merged() {
    let e = env();
    let mut http = sor();
    edit_products(&mut http, |r| {
        let p = r.iter_mut().find(|p| p["productId"] == 540198).unwrap();
        p["name"] = json!("Obi-Wan Kenobi - Patient Mentor");
    });
    sync_official(&e, &http, at(13, 4));
    sync_tcgcsv(&e, &http, at(13, 4));
    let c = Catalog::open_readonly(&e.db).unwrap();
    let normal = uid_of(c.conn(), "SOR", "77", "Standard", "Normal");
    assert!(alarms(c.conn(), "official_without_tcgcsv").contains(&normal));
    assert!(alarms(c.conn(), "tcgcsv_without_official").contains(&"540198".to_string()));
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM printing_source_id WHERE source='tcgcsv' AND source_id='540198'"
        ),
        0
    );
}

#[test]
fn a_group_that_maps_to_no_official_set_raises_an_alarm() {
    let e = env();
    let mut http = sor();
    let mut v: Value = serde_json::from_slice(&http.products[&23405]).unwrap();
    for p in v["results"].as_array_mut().unwrap() {
        p["groupId"] = json!(24812);
        p["productId"] = json!(p["productId"].as_i64().unwrap() + 1_000_000);
    }
    http.products.insert(24812, serde_json::to_vec(&v).unwrap());
    sync_official(&e, &http, at(13, 4));
    sync_tcgcsv(&e, &http, at(13, 4));
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(
        alarms(c.conn(), "unmapped_group"),
        vec!["24812".to_string()]
    );
}

#[test]
fn an_unchanged_last_updated_stamp_fetches_nothing_else() {
    let (e, http) = loaded();
    http.asked.borrow_mut().clear();
    let r = sync_tcgcsv(&e, &http, at(13, 9));
    assert!(r.is_empty());
    let asked = http.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].ends_with("/last-updated.txt"));
}

#[test]
fn the_tcgcsv_calls_follow_the_prescribed_order() {
    let e = env();
    let http = sor();
    sync_official(&e, &http, at(13, 4));
    http.asked.borrow_mut().clear();
    sync_tcgcsv(&e, &http, at(13, 4));
    let asked = http.asked.borrow();
    assert!(asked[0].ends_with("/last-updated.txt"));
    assert!(asked[1].ends_with("/tcgplayer/79/groups"));
    assert!(asked[2].ends_with("/products") && asked[3].ends_with("/prices"));
}

// ---- the skipped day -------------------------------------------------------

fn priced_on(e: &Env, http: &Recorded, stamp: &str, now: DateTime<Utc>) {
    let http = Recorded {
        last_updated: stamp.into(),
        official_pages: http.official_pages.clone(),
        groups: http.groups.clone(),
        products: http.products.clone(),
        prices: http.prices.clone(),
        asked: RefCell::new(vec![]),
    };
    sync_tcgcsv(e, &http, now);
}

#[test]
fn a_skipped_price_day_raises_an_alarm_within_the_next_check_window() {
    let e = env();
    let http = sor();
    sync_official(&e, &http, at(10, 4));
    priced_on(&e, &http, "2026-10-10T20:05:44+0000", at(10, 21));
    priced_on(&e, &http, "2026-10-11T20:05:44+0000", at(11, 21));
    // 12 October never arrives; 13 October does.
    priced_on(&e, &http, "2026-10-13T20:05:44+0000", at(13, 21));
    let c = Catalog::open_readonly(&e.db).unwrap();
    let missing: Vec<String> = check::missing_price_days(c.conn(), at(13, 23))
        .unwrap()
        .into_iter()
        .map(|a| a.key)
        .collect();
    assert_eq!(missing, vec!["2026-10-12".to_string()]);
    assert!(
        check::run(c.conn(), at(13, 23))
            .unwrap()
            .iter()
            .any(|a| a.kind == "missing_price_day")
    );

    // The window: a day is due once the sync has had its hour, not before.
    let c2 = env();
    sync_official(&c2, &http, at(10, 4));
    priced_on(&c2, &http, "2026-10-11T20:05:44+0000", at(11, 21));
    let r = Catalog::open_readonly(&c2.db).unwrap();
    assert!(
        check::missing_price_days(r.conn(), at(12, 21))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        check::missing_price_days(r.conn(), at(12, 22))
            .unwrap()
            .len(),
        1
    );
    // A day that did arrive, checked an hour after its sync, is quiet.
    priced_on(&c2, &http, "2026-10-12T20:05:44+0000", at(12, 21));
    let r = Catalog::open_readonly(&c2.db).unwrap();
    assert!(
        check::missing_price_days(r.conn(), at(12, 23))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_thin_price_day_raises_a_coverage_alarm() {
    let (e, mut http) = loaded();
    // Next day tcgcsv serves almost no prices.
    let mut v: Value = serde_json::from_slice(&http.prices[&23405]).unwrap();
    v["results"].as_array_mut().unwrap().truncate(10);
    http.prices.insert(23405, serde_json::to_vec(&v).unwrap());
    http.prices.insert(23451, br#"{"results":[]}"#.to_vec());
    priced_on(&e, &http, "2026-10-13T20:05:44+0000", at(13, 21));
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert!(
        check::run(c.conn(), at(13, 23))
            .unwrap()
            .iter()
            .any(|a| a.kind == "low_price_coverage")
    );
}

// ---- sp-5zcm06: two official printings that share a display variant ---------

#[test]
fn two_printings_sharing_set_number_variant_name_and_finish_both_apply() {
    use dumpstar_catalog::apply::apply;
    use dumpstar_catalog::landing::{Provenance, sha256_hex};
    let bytes = read("off-sec-shared-standard.json");
    let dir = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    let prov = Provenance {
        source: "official".into(),
        url: "https://official.test/api/cards?page=1".into(),
        fetched_at: "2026-10-13T00:00:00Z".into(),
        sha256: sha256_hex(&bytes),
        bytes: bytes.len() as u64,
        as_of: None,
    };
    let r = apply(&mut catalog, &bytes, &prov).unwrap();
    assert_eq!(r.printings, 2);
    let conn = catalog.conn();
    let mut s = conn
        .prepare(
            "SELECT printing_id, set_code, number, variant, variant_code, finish
             FROM printings ORDER BY printing_id",
        )
        .unwrap();
    let rows: Vec<(String, String, String, String, String, String)> = s
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let row = |uid: &str, code: &str| {
        (
            uid.to_string(),
            "SEC".to_string(),
            "1125".to_string(),
            "Standard".to_string(),
            code.to_string(),
            "Normal".to_string(),
        )
    };
    assert_eq!(rows, vec![row("9911251036", "36"), row("9911251037", "37")]);
}

// ---- release dates ----------------------------------------------------------

#[test]
fn the_recorded_load_carries_each_sets_release_date_and_names_a_set_without_one() {
    let (e, _) = loaded();
    let c = Catalog::open_readonly(&e.db).unwrap();
    let date: Option<String> = c
        .conn()
        .query_row(
            "SELECT release_date FROM sets WHERE set_code = 'SOR'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(date.as_deref(), Some("2024-03-08"));
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM sets WHERE release_date IS NULL"
        ),
        0
    );
    assert!(alarms(c.conn(), "set_without_release_date").is_empty());

    // A later official refresh (which carries no date) must not blank it.
    let http = sor();
    sync_official(&e, &http, at(14, 4));
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(
        count(
            c.conn(),
            "SELECT COUNT(*) FROM sets WHERE release_date = '2024-03-08'"
        ),
        1
    );

    // A set no source dates is reported by name, not left silently NULL.
    let w = Catalog::open_writer(&e.db).unwrap();
    w.conn()
        .execute(
            "INSERT INTO sets (set_code, name, sort_order) VALUES ('ZZZ', 'Undated', 0)",
            [],
        )
        .unwrap();
    drop(w);
    sync_tcgcsv(
        &e,
        &Recorded {
            last_updated: "2026-10-15T00:00:00".into(),
            ..sor()
        },
        at(15, 4),
    );
    let c = Catalog::open_readonly(&e.db).unwrap();
    assert_eq!(alarms(c.conn(), "set_without_release_date"), vec!["ZZZ"]);
}

#[test]
fn a_set_no_source_dates_takes_its_date_from_the_remedial_dataset() {
    let (e, _) = loaded();
    let w = Catalog::open_writer(&e.db).unwrap();
    w.conn()
        .execute(
            "INSERT INTO sets (set_code, name, sort_order) VALUES ('SHD', 'Shadows of the Galaxy', 1)",
            [],
        )
        .unwrap();
    drop(w);
    sync_tcgcsv(
        &e,
        &Recorded {
            last_updated: "2026-10-15T00:00:00".into(),
            ..sor()
        },
        at(15, 4),
    );
    let c = Catalog::open_readonly(&e.db).unwrap();
    let date: Option<String> = c
        .conn()
        .query_row(
            "SELECT release_date FROM sets WHERE set_code = 'SHD'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(date.as_deref(), Some("2024-07-12"));
    assert!(alarms(c.conn(), "set_without_release_date").is_empty());
}
