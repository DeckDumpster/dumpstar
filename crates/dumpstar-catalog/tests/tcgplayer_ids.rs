//! Every printing TCGplayer sells carries its TCGplayer product id, so its
//! price (sp-9uiu2z), on recorded tcgcsv payloads for Star Wars: Unlimited
//! (category 79) and the official catalog of the six sets Jump to Lightspeed
//! through Homeworlds. The recordings are the full tcgcsv products of those
//! sets and the official entries trimmed to the fields the parser reads
//! (`fixtures/swu/README.md`).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use dumpstar_catalog::crosswalk::{Extra, OfficialPrinting, Remedial, join, join_with};
use dumpstar_catalog::fetch::Fetch;
use dumpstar_catalog::job::run_once;
use dumpstar_catalog::swu::{OfficialSource, TcgcsvSource};
use dumpstar_catalog::tcgcsv::{GroupInfo, GroupKind, ProductRecord};
use dumpstar_catalog::{Catalog, check};
use rusqlite::Connection;

const MAIN_SETS: [&str; 6] = ["JTL", "LOF", "SEC", "LAW", "ASH", "HMW"];
const COSTCO_LEIA: &str = "8379900881";

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

struct Recorded {
    official_pages: Vec<Vec<u8>>,
    products: BTreeMap<i64, Vec<u8>>,
    prices: BTreeMap<i64, Vec<u8>>,
}

impl Fetch for Recorded {
    fn get(&self, url: &str) -> dumpstar_catalog::Result<Vec<u8>> {
        if let Some(rest) = url.split("pagination[page]=").nth(1) {
            let n: usize = rest.split('&').next().unwrap().parse().unwrap();
            return Ok(self.official_pages[n - 1].clone());
        }
        if url.ends_with("last-updated.txt") {
            return Ok(read("last-updated.txt"));
        }
        if url.ends_with("/groups") {
            return Ok(read("swu-groups.json"));
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

fn recorded() -> Recorded {
    let mut products = BTreeMap::new();
    let mut prices = BTreeMap::new();
    for (name, gid) in [
        ("jtl", 23956),
        ("lof", 24279),
        ("sec", 24387),
        ("law", 24572),
        ("ash", 24660),
        ("hmw", 24812),
    ] {
        products.insert(gid, read(&format!("tcgcsv-{name}-{gid}-products.json.gz")));
    }
    prices.insert(24660, read("tcgcsv-ash-24660-prices.json.gz"));
    let mut sets: Vec<&str> = MAIN_SETS.to_vec();
    sets.push("CST");
    Recorded {
        official_pages: sets
            .iter()
            .map(|s| read(&format!("official-trim-{s}.json.gz")))
            .collect(),
        products,
        prices,
    }
}

struct Loaded {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

fn loaded() -> Loaded {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw");
    let db = dir.path().join("shared.sqlite");
    let http = recorded();
    let now = at(13, 4);
    let official = OfficialSource {
        http: &http,
        base: "https://official.test".into(),
        since: None,
        crawled_on: None,
        today: "2026-10-13".into(),
    };
    run_once(&official, &raw, &db, now).unwrap();
    let tcgcsv = TcgcsvSource {
        http: &http,
        base: "https://tcgcsv.test".into(),
        last_updated: None,
    };
    run_once(&tcgcsv, &raw, &db, now).unwrap();
    Loaded { _dir: dir, db }
}

fn rows<T>(conn: &Connection, sql: &str, f: impl Fn(&rusqlite::Row) -> T) -> Vec<T> {
    let mut s = conn.prepare(sql).unwrap();
    s.query_map([], |r| Ok(f(r)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn the_costco_leia_is_linked_to_its_tcgplayer_product_and_gets_its_market_price() {
    let l = loaded();
    let c = Catalog::open_readonly(&l.db).unwrap();
    let ids: Vec<(String, String)> = rows(
        c.conn(),
        &format!(
            "SELECT source_id, sub_type_name FROM printing_source_id
             WHERE source = 'tcgcsv' AND printing_id = '{COSTCO_LEIA}'"
        ),
        |r| (r.get(0).unwrap(), r.get(1).unwrap()),
    );
    assert_eq!(ids, vec![("719791".to_string(), "Foil".to_string())]);
    let market: f64 = c
        .conn()
        .query_row(
            "SELECT market FROM prices WHERE printing_id = ?1 AND source = 'tcgcsv'
             AND observed_on = '2026-10-12'",
            [COSTCO_LEIA],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(market, 10.38);
}

#[test]
fn every_main_set_has_at_least_95_percent_of_its_printings_carrying_a_product_id() {
    let l = loaded();
    let c = Catalog::open_readonly(&l.db).unwrap();
    let cov = check::set_coverage(c.conn()).unwrap();
    for set in MAIN_SETS {
        let s = cov.iter().find(|s| s.set_code == set).unwrap();
        assert!(s.percent() >= 95, "{set}: {s:?}");
    }
    let s = cov.iter().find(|s| s.set_code == "CST").unwrap();
    assert_eq!((s.linked, s.printings), (2, 2));
}

#[test]
fn the_coverage_report_names_every_printing_left_without_a_product_id() {
    let l = loaded();
    let c = Catalog::open_readonly(&l.db).unwrap();
    let missing: i64 = c
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM printings p WHERE NOT EXISTS (SELECT 1 FROM printing_source_id s
               WHERE s.printing_id = p.printing_id AND s.source = 'tcgcsv')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let report = check::unmatched(c.conn()).unwrap();
    assert!(missing > 0);
    assert_eq!(report.len() as i64, missing);
    let ids: Vec<String> = rows(
        c.conn(),
        "SELECT printing_id FROM printings p WHERE NOT EXISTS (SELECT 1 FROM printing_source_id s
           WHERE s.printing_id = p.printing_id AND s.source = 'tcgcsv')",
        |r| r.get(0).unwrap(),
    );
    for id in ids {
        assert!(
            report.iter().any(|l| l.contains(&format!("[{id}]"))),
            "{id}"
        );
    }
}

#[test]
fn a_set_whose_coverage_falls_below_its_floor_fails_the_check() {
    let l = loaded();
    let c = Catalog::open_writer(&l.db).unwrap();
    assert!(
        check::coverage_drops(c.conn()).unwrap().is_empty(),
        "{:?}",
        check::coverage_drops(c.conn()).unwrap()
    );
    c.conn()
        .execute(
            "DELETE FROM printing_source_id WHERE source = 'tcgcsv' AND printing_id IN
               (SELECT printing_id FROM printings WHERE set_code = 'ASH' AND finish = 'Foil')",
            [],
        )
        .unwrap();
    let drops = check::coverage_drops(c.conn()).unwrap();
    assert_eq!(drops.len(), 1, "{drops:?}");
    assert_eq!(
        (drops[0].kind.as_str(), drops[0].key.as_str()),
        ("set_coverage_dropped", "ASH")
    );
    let all = check::run(c.conn(), at(13, 23)).unwrap();
    assert!(all.iter().any(|a| a.kind == "set_coverage_dropped"));
}

#[test]
fn a_foil_and_a_prestige_listing_are_matched_across_the_group_by_name_and_parenthetical() {
    let l = loaded();
    let c = Catalog::open_readonly(&l.db).unwrap();
    // Leia, Vigilant for Danger in Ashes of the Empire: six printings, each its
    // own TCGplayer product, none of them numbered like the collector number.
    let linked: Vec<(String, String, String)> = rows(
        c.conn(),
        "SELECT p.variant, p.finish, s.source_id FROM printings p
           JOIN printing_source_id s ON s.printing_id = p.printing_id AND s.source = 'tcgcsv'
           JOIN cards c ON c.card_id = p.card_id
          WHERE c.name = 'Leia Organa' AND c.subtitle = 'Vigilant for Danger' AND p.set_code = 'ASH'
          ORDER BY s.source_id",
        |r| (r.get(0).unwrap(), r.get(1).unwrap(), r.get(2).unwrap()),
    );
    let products: Vec<&str> = linked.iter().map(|l| l.2.as_str()).collect();
    assert_eq!(
        products,
        vec!["688989", "703294", "703547", "704230", "704277", "704324"],
        "{linked:?}"
    );
}

fn printing(id: &str, name: &str) -> OfficialPrinting {
    OfficialPrinting {
        printing_id: id.into(),
        set_code: "LAW".into(),
        number: "79".into(),
        variant: "Standard".into(),
        variant_code: "01".into(),
        finish: "Normal".into(),
        name: name.into(),
        subtitle: Some("Locking the Vault".into()),
    }
}

#[test]
fn a_printing_the_rules_cannot_place_is_resolved_by_the_remedial_crosswalk() {
    let groups = BTreeMap::from([(
        1,
        GroupInfo {
            set_code: "LAW".into(),
            kind: GroupKind::Main,
            variants: vec![],
        },
    )]);
    let products = [ProductRecord {
        product_id: 678656,
        group_id: 1,
        name: "K-2S0 - Locking the Vault".into(),
        number: Some("79/264".into()),
    }];
    let printings = [printing("k2so", "K-2SO")];
    let none = BTreeMap::new();

    let j = join(&printings, &products, &groups, &none);
    assert!(j.links.is_empty(), "{:?}", j.links);

    let remedial = [Remedial {
        printing_id: "k2so".into(),
        product_id: 678656,
        source: "manual".into(),
        reason: "TCGplayer spells the name K-2S0".into(),
    }];
    let extra = Extra {
        remedial: &remedial,
        ..Default::default()
    };
    let j = join_with(&printings, &products, &groups, &none, &extra);
    let linked: Vec<(&str, i64)> = j
        .links
        .iter()
        .map(|l| (l.printing_id.as_str(), l.product_id))
        .collect();
    assert_eq!(linked, vec![("k2so", 678656)]);

    // A product tcgcsv stopped listing is an alarm, not a silent miss.
    let stale = [Remedial {
        product_id: 1,
        ..remedial[0].clone()
    }];
    let extra = Extra {
        remedial: &stale,
        ..Default::default()
    };
    let j = join_with(&printings, &products, &groups, &none, &extra);
    assert!(
        j.alarms.iter().any(|a| a.kind == "remedial_stale"),
        "{:?}",
        j.alarms
    );
}

#[test]
fn every_row_of_the_remedial_crosswalk_names_a_source_and_a_reason() {
    let table = dumpstar_catalog::crosswalk::remedial_table();
    assert!(!table.is_empty());
    for r in &table {
        assert!(!r.source.is_empty() && !r.reason.is_empty(), "{r:?}");
    }
    let mut ids: Vec<&str> = table.iter().map(|r| r.printing_id.as_str()).collect();
    ids.sort();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "a printing is resolved once");
}
