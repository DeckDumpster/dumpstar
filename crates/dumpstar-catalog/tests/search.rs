//! Acceptance for catalog search (design row S11).

use chrono::{TimeZone, Utc};
use dumpstar_catalog::job::run_once;
use dumpstar_catalog::search::{QueryError, SearchError, search};
use dumpstar_catalog::{Catalog, FakeSource};

fn built() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("shared.sqlite");
    let now = Utc.with_ymd_and_hms(2026, 10, 13, 4, 0, 0).unwrap();
    run_once(&FakeSource, &dir.path().join("raw"), &db, now).unwrap();
    (dir, db)
}

fn ids(db: &std::path::Path, q: &str) -> Vec<String> {
    let c = Catalog::open_readonly(db).unwrap();
    search(&c, q)
        .unwrap()
        .into_iter()
        .map(|h| h.printing_id)
        .collect()
}

#[test]
fn attributes_and_text_filter_printings() {
    let (_d, db) = built();
    assert_eq!(
        ids(&db, "type:leader"),
        ["2001:Foil", "2001:Normal", "2002:Foil", "2002:Normal"]
    );
    let falcon = ids(&db, "falcon");
    assert!(!falcon.is_empty());
    assert!(falcon.iter().all(|id| !id.starts_with("2001")));
    assert_eq!(ids(&db, "title:\"millennium falcon\""), falcon);
    assert_eq!(ids(&db, "arena:space trait:vehicle cost:3"), falcon);
    assert_eq!(ids(&db, "aspect:CUNNING cost>=3 cost<4"), falcon);
    assert!(ids(&db, "falcon -arena:space").is_empty());
    assert!(
        ids(&db, "set:SOR variant:hyperspace finish:foil")
            .iter()
            .all(|id| id.ends_with("Foil"))
    );
    assert!(!ids(&db, "set:SOR variant:hyperspace finish:foil").is_empty());
    assert!(ids(&db, "cost:99").is_empty());
}

#[test]
fn the_index_is_read_not_written_by_a_search() {
    let (_d, db) = built();
    let c = Catalog::open_readonly(&db).unwrap();
    let a = search(&c, "rebel").unwrap();
    // identical rows however many askers: the query takes no tenant
    assert_eq!(
        a,
        search(&Catalog::open_readonly(&db).unwrap(), "rebel").unwrap()
    );
    assert!(
        c.conn()
            .execute("INSERT INTO card_search(card_id,name) VALUES('x','x')", [])
            .is_err()
    );
}

#[test]
fn bad_queries_are_refused_and_fts_syntax_is_inert() {
    let (_d, db) = built();
    let c = Catalog::open_readonly(&db).unwrap();
    assert!(matches!(
        search(&c, "bogus:1"),
        Err(SearchError::Query(QueryError::UnknownField(_)))
    ));
    assert!(matches!(
        search(&c, "cost:abc"),
        Err(SearchError::Query(QueryError::NotANumber(..)))
    ));
    assert!(matches!(
        search(&c, "\"open"),
        Err(SearchError::Query(QueryError::Unterminated))
    ));
    search(&c, "NEAR( OR AND * ^").unwrap();
}
