//! A `shared.sqlite` written by the Pokémon-shaped schema dumpstar was forked
//! with is detected and rebuilt, not failed on (sp-r7w5xi).

use dumpstar_catalog::job::run_once;
use dumpstar_catalog::{Catalog, FakeSource};
use rusqlite::Connection;

fn legacy_file(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("shared.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch(dumpstar_catalog::LEGACY_SHAPE_DDL)
        .unwrap();
    path
}

#[test]
fn open_writer_rebuilds_a_legacy_shaped_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = legacy_file(dir.path());
    assert!(dumpstar_catalog::is_legacy(&Connection::open(&path).unwrap()).unwrap());

    let catalog = Catalog::open_writer(&path).unwrap();
    assert!(catalog.rebuilt_legacy);
    let conn = catalog.conn();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('cards')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(cols.contains(&"aspects".to_string()));
    assert!(!dumpstar_catalog::is_legacy(conn).unwrap());
    let gone: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM sqlite_master WHERE name IN ({})",
                dumpstar_catalog::LEGACY_ONLY_OBJECTS
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        gone, 0,
        "views, triggers and virtual tables are dropped too"
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM sets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn catalog_apply_over_a_legacy_file_succeeds_and_a_second_open_keeps_the_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = legacy_file(dir.path());
    let raw = dir.path().join("raw");
    run_once(&FakeSource, &raw, &path, chrono::Utc::now()).unwrap();

    let catalog = Catalog::open_writer(&path).unwrap();
    assert!(
        !catalog.rebuilt_legacy,
        "an SWU-shaped file is never rebuilt"
    );
    let n: i64 = catalog
        .conn()
        .query_row("SELECT count(*) FROM cards", [], |r| r.get(0))
        .unwrap();
    assert!(n > 0, "the apply landed cards");
}

/// The SWU schema before sp-5zcm06: `printings` keyed on
/// (set_code, number, variant, finish), no `variant_code`.
const PRE_VARIANT_CODE_PRINTINGS: &str = "
CREATE TABLE sets (set_code TEXT PRIMARY KEY, name TEXT NOT NULL, release_date TEXT,
                   sort_order INTEGER NOT NULL DEFAULT 0);
CREATE TABLE cards (card_id TEXT PRIMARY KEY, name TEXT NOT NULL, subtitle TEXT);
CREATE TABLE printings (printing_id TEXT PRIMARY KEY, card_id TEXT NOT NULL REFERENCES cards(card_id),
                        set_code TEXT NOT NULL REFERENCES sets(set_code), number TEXT NOT NULL,
                        variant TEXT NOT NULL, finish TEXT NOT NULL, rarity TEXT, artist TEXT,
                        image_url TEXT, UNIQUE (set_code, number, variant, finish));
INSERT INTO sets VALUES ('SEC', 'Secrets of Power', NULL, 0);
INSERT INTO cards VALUES ('c1', 'Senator Chuchi', NULL);
INSERT INTO printings VALUES ('u1', 'c1', 'SEC', '1125', 'Standard', 'Normal', NULL, NULL, NULL);
";

#[test]
fn a_file_from_the_swu_schema_before_variant_code_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch(PRE_VARIANT_CODE_PRINTINGS)
        .unwrap();
    assert!(dumpstar_catalog::is_legacy(&Connection::open(&path).unwrap()).unwrap());

    let catalog = Catalog::open_writer(&path).unwrap();
    assert!(catalog.rebuilt_legacy);
    let has_code: bool = catalog
        .conn()
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('printings') WHERE name = 'variant_code')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_code);
    assert!(!dumpstar_catalog::is_legacy(catalog.conn()).unwrap());
    drop(catalog);
    assert!(!Catalog::open_writer(&path).unwrap().rebuilt_legacy);
}
