//! A `shared.sqlite` written by an earlier card schema is detected and rebuilt, not failed on (sp-r7w5xi).

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
