//! Acceptance for the SWU catalog seam (design row S6).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use dumpstar_swu::apply::apply;
use dumpstar_swu::job::run_once;
use dumpstar_swu::landing::{Provenance, land, sha256_hex};
use dumpstar_swu::source::{FAKE_CATALOG, FAKE_URL};
use dumpstar_swu::{Catalog, FakeSource};
use rusqlite::Connection;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 13, 4, 0, 0).unwrap()
}

/// Every row of every catalog table, as text, in a stable order: the
/// catalog's whole content in one comparable value.
fn dump(conn: &Connection) -> BTreeMap<String, Vec<String>> {
    let tables = [
        "sets",
        "cards",
        "printings",
        "printing_source_id",
        "prices",
        "formats",
        "format_rules",
        "card_legality",
        "card_search",
        "applied_payloads",
    ];
    let mut out = BTreeMap::new();
    for t in tables {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok((0..n)
                    .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                    .collect::<Vec<_>>()
                    .join("|"))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows.sort();
        out.insert(t.to_string(), rows);
    }
    out
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
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

#[test]
fn applying_the_fake_catalog_twice_is_a_noop_the_second_time() {
    let e = env();
    let first = run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    assert!(!first[0].applied.is_noop(), "first apply must write rows");
    assert_eq!(first[0].applied.printings, 7);
    let after_first = dump(Catalog::open_readonly(&e.db).unwrap().conn());

    let second = run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    assert!(
        second[0].applied.is_noop(),
        "second apply wrote: {:?}",
        second[0].applied
    );
    assert_eq!(
        after_first,
        dump(Catalog::open_readonly(&e.db).unwrap().conn())
    );
}

#[test]
fn identical_content_under_a_new_hash_writes_nothing() {
    let e = env();
    run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    let before = dump(Catalog::open_readonly(&e.db).unwrap().conn());

    // Same catalog, different bytes (whitespace) → a different hash.
    let reserialised = FAKE_CATALOG.replace("{\n", "{\n ");
    assert_ne!(
        sha256_hex(reserialised.as_bytes()),
        sha256_hex(FAKE_CATALOG.as_bytes())
    );
    let prov = Provenance {
        source: "fake".into(),
        url: FAKE_URL.into(),
        fetched_at: now().to_rfc3339(),
        sha256: sha256_hex(reserialised.as_bytes()),
        bytes: 0,
        as_of: None,
    };
    let mut c = Catalog::open_writer(&e.db).unwrap();
    let report = apply(&mut c, reserialised.as_bytes(), &prov).unwrap();
    assert!(!report.already_applied);
    assert!(report.is_noop(), "row-level idempotence failed: {report:?}");
    let mut after = dump(c.conn());
    // The only permitted difference is the new payload's provenance row.
    after.get_mut("applied_payloads").unwrap().clear();
    let mut before = before;
    before.get_mut("applied_payloads").unwrap().clear();
    assert_eq!(before, after);
}

#[test]
fn raw_payload_is_on_disk_with_hash_and_provenance_before_apply() {
    let e = env();
    let r = run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    let landed = &r[0].landed;
    let on_disk = fs::read(&landed.path).unwrap();
    assert_eq!(on_disk, FAKE_CATALOG.as_bytes());
    assert_eq!(landed.provenance.sha256, sha256_hex(&on_disk));
    assert!(
        landed
            .path
            .ends_with(format!("fake/2026-10-13/{}.json", landed.provenance.sha256))
    );
    let meta: Provenance = serde_json::from_slice(
        &fs::read(
            landed
                .path
                .with_file_name(format!("{}.meta.json", landed.provenance.sha256)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(meta.url, FAKE_URL);
    assert_eq!(meta.fetched_at, "2026-10-13T04:00:00+00:00");
    assert_eq!(meta, landed.provenance);
}

#[test]
fn a_payload_that_fails_to_parse_is_still_landed() {
    struct Bad;
    impl dumpstar_swu::CatalogSource for Bad {
        fn fetch(&self) -> dumpstar_swu::Result<Vec<dumpstar_swu::RawPayload>> {
            Ok(vec![dumpstar_swu::RawPayload {
                source: "fake".into(),
                url: "fake://bad".into(),
                bytes: b"not json".to_vec(),
                as_of: None,
            }])
        }
    }
    let e = env();
    assert!(run_once(&Bad, &e.raw, &e.db, now()).is_err());
    let day = e.raw.join("fake/2026-10-13");
    let landed: Vec<_> = fs::read_dir(&day).unwrap().collect();
    assert_eq!(
        landed.len(),
        2,
        "payload and its provenance must survive a parse failure"
    );
}

fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for ent in fs::read_dir(d).unwrap() {
            let p = ent.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn no_tenant_file_changes_across_a_run() {
    let e = env();
    let tenants = e._dir.path().join("tenants");
    fs::create_dir_all(&tenants).unwrap();
    for (name, body) in [
        ("ryan.sqlite", "tenant-a bytes"),
        ("friend.sqlite", "tenant-b bytes"),
    ] {
        fs::write(tenants.join(name), body).unwrap();
    }
    let before = tree_bytes(&tenants);
    run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    assert_eq!(before, tree_bytes(&tenants));
}

#[test]
fn every_printing_kind_is_its_own_row_with_crosswalk() {
    let e = env();
    run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    let c = Catalog::open_readonly(&e.db).unwrap();
    let c = c.conn();
    assert_eq!(
        count(c, "SELECT COUNT(*) FROM printings WHERE card_id='1001'"),
        4
    );
    assert_eq!(
        count(c, "SELECT COUNT(*) FROM printings WHERE variant='Showcase'"),
        1
    );
    assert_eq!(
        count(c, "SELECT COUNT(*) FROM printings WHERE variant='Promo'"),
        1
    );
    let foil: String = c
        .query_row(
            "SELECT printing_id FROM printing_source_id WHERE source='tcgcsv' AND source_id='900001' AND sub_type_name='Foil'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(foil, "2001:Foil");
    assert_eq!(
        count(
            c,
            "SELECT COUNT(*) FROM printing_source_id WHERE source='official' AND source_id='2001'"
        ),
        2
    );
}

#[test]
fn prices_are_append_only_and_unmatched_rows_are_counted() {
    let e = env();
    let r = run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    assert_eq!(r[0].applied.prices, 4);
    assert_eq!(r[0].applied.unmatched_prices, 1);
    let w = Catalog::open_writer(&e.db).unwrap();
    assert!(
        w.conn()
            .execute("UPDATE prices SET market = 0", [])
            .is_err()
    );
    assert!(w.conn().execute("DELETE FROM prices", []).is_err());
    assert_eq!(count(w.conn(), "SELECT COUNT(*) FROM prices"), 4);
}

#[test]
fn fts_and_legality_are_populated() {
    let e = env();
    run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    let c = Catalog::open_readonly(&e.db).unwrap();
    let c = c.conn();
    let id: String = c
        .query_row(
            "SELECT card_id FROM card_search WHERE card_search MATCH 'falcon'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(id, "1002");
    assert_eq!(
        count(
            c,
            "SELECT COUNT(*) FROM card_legality WHERE status='banned'"
        ),
        1
    );
    assert_eq!(
        count(
            c,
            "SELECT COUNT(*) FROM format_rules WHERE format_id='premier'"
        ),
        3
    );
}

#[test]
fn the_serving_connection_cannot_write() {
    let e = env();
    run_once(&FakeSource, &e.raw, &e.db, now()).unwrap();
    let ro = Catalog::open_readonly(&e.db).unwrap();
    assert!(ro.conn().execute("DELETE FROM cards", []).is_err());
}

/// The serving crate must not reach the write path. `Catalog::open_writer`,
/// `apply` and `run_once` are the only ways to write; none may appear in
/// dumpstar-server or in a crate it serves requests from.
#[test]
fn no_request_handler_opens_the_catalog_for_writing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dumpstar-server/src");
    let mut stack = vec![root];
    let mut offenders = Vec::new();
    while let Some(d) = stack.pop() {
        for ent in fs::read_dir(d).unwrap() {
            let p = ent.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                // Production code only: a test legitimately builds the catalog
                // it serves from (as `dumpstar-server/tests/no_catalog_writer.rs`
                // also allows).
                let text = fs::read_to_string(&p).unwrap();
                let src = match text.find("#[cfg(test)]") {
                    Some(i) => &text[..i],
                    None => &text,
                };
                if src.contains("dumpstar_swu")
                    && ["open_writer", "apply", "run_once", "job::"]
                        .iter()
                        .any(|w| src.contains(w))
                {
                    offenders.push(p);
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "request path reaches the catalog write API: {offenders:?}"
    );
}

#[test]
fn landing_twice_keeps_the_first_provenance() {
    let e = env();
    let p = dumpstar_swu::RawPayload {
        source: "fake".into(),
        url: "fake://x".into(),
        bytes: b"{}".to_vec(),
        as_of: None,
    };
    let a = land(&e.raw, &p, now()).unwrap();
    let later = now() + chrono::Duration::hours(1);
    let b = land(&e.raw, &p, later).unwrap();
    assert_eq!(a.path, b.path);
    let meta: Provenance = serde_json::from_slice(
        &fs::read(
            a.path
                .with_file_name(format!("{}.meta.json", a.provenance.sha256)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(meta.fetched_at, a.provenance.fetched_at);
}
