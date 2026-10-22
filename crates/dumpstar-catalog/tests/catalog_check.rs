//! What `catalog-check` fails on and what it lets pass (sp-h41oq8): a corrected
//! check still fires on a planted defect of each kind it was corrected for, and an
//! accepted alarm is exactly the one the table names.

use chrono::{TimeZone, Utc};
use dumpstar_catalog::{Catalog, check};

fn catalog() -> (tempfile::TempDir, Catalog) {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    (dir, c)
}

fn kinds(a: Vec<dumpstar_catalog::record::AlarmRecord>) -> Vec<(String, String)> {
    a.into_iter().map(|a| (a.kind, a.key)).collect()
}

const MAIN: &str = "INSERT INTO sets (set_code, name, category, release_date) VALUES ('JTL','Jump to Lightspeed','Main Expansions','2025-03-14');
     INSERT INTO cards (card_id, name) VALUES ('c','Card');";

fn printing(id: &str, number: u32, variant: &str, code: &str, finish: &str) -> String {
    format!(
        "INSERT INTO printings (printing_id, card_id, set_code, number, variant, variant_code, finish)
         VALUES ('{id}','c','JTL','{number}','{variant}','{code}','{finish}');"
    )
}

#[test]
fn foils_and_serialized_parallels_are_not_out_of_range_but_a_stray_normal_is() {
    let (_d, c) = catalog();
    c.conn().execute_batch(MAIN).unwrap();
    for n in 1..=10 {
        c.conn()
            .execute_batch(&printing(&format!("n{n}"), n, "Standard", "01", "Normal"))
            .unwrap();
    }
    // The official API numbers a Standard Foil past the printed count, and files a
    // Rose Gold serialized parallel as Standard at a Prestige number.
    c.conn()
        .execute_batch(&printing("f", 525, "Standard", "31", "Foil"))
        .unwrap();
    c.conn()
        .execute_batch(&printing("g", 1125, "Standard", "36", "Normal"))
        .unwrap();
    assert!(check::datalint(c.conn()).unwrap().is_empty());
    assert!(check::numbering(c.conn()).unwrap().is_empty());
    // A promo run filed in the set at a number of its own still alarms.
    c.conn()
        .execute_batch(&printing("x", 900, "Standard", "99", "Normal"))
        .unwrap();
    assert_eq!(
        kinds(check::datalint(c.conn()).unwrap()),
        [("number_out_of_range".into(), "JTL/900".into())]
    );
    assert_eq!(
        kinds(check::numbering(c.conn()).unwrap()),
        [("standard_numbering_gap".into(), "JTL".into())]
    );
}

#[test]
fn a_clause_said_twice_in_one_line_is_a_card_but_a_line_said_twice_is_a_defect() {
    let (_d, c) = catalog();
    for (id, text) in [
        (
            "choice",
            "Choose one:\n* It costs 1 resource less.\n* Exhaust a unit. It costs 1 resource less.",
        ),
        (
            "twice",
            "Each friendly damaged unit gets +1/+0.\nEach friendly damaged unit gets +1/+0.",
        ),
    ] {
        c.conn()
            .execute(
                "INSERT INTO cards (card_id, name, card_type, rules_text) VALUES (?1, ?1, 'Event', ?2)",
                rusqlite::params![id, text],
            )
            .unwrap();
    }
    let got = check::suspect_rules_text(c.conn()).unwrap();
    assert_eq!(kinds(got), [("rules_text_repeated".into(), "twice".into())]);
}

fn alarm(c: &Catalog, kind: &str, key: &str, severity: &str) {
    c.conn()
        .execute(
            "INSERT INTO alarms (kind, key, severity, detail) VALUES (?1,?2,?3,'d')",
            rusqlite::params![kind, key, severity],
        )
        .unwrap();
}

#[test]
fn an_accepted_alarm_passes_a_new_key_of_its_kind_fails_and_a_note_never_does() {
    let (_d, c) = catalog();
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let (kind, key, reason) = check::accepted_table().remove(0);
    alarm(&c, &kind, &key, "alarm");
    alarm(&c, &kind, "not-in-the-table", "alarm");
    c.conn()
        .execute(
            "INSERT INTO cards (card_id, name, card_type, rules_text) VALUES ('k','K','Event','')",
            [],
        )
        .unwrap();
    let r = check::run_report(c.conn(), now).unwrap();
    assert_eq!(kinds(r.open), [(kind.clone(), "not-in-the-table".into())]);
    assert_eq!(r.accepted.len(), 1);
    assert_eq!(r.accepted[0].1, reason);
    assert_eq!(r.info.len(), 1);
}

#[test]
fn a_printing_tcgcsv_lacks_is_expected_only_before_its_set_is_on_sale() {
    let (_d, c) = catalog();
    c.conn()
        .execute_batch(
            "INSERT INTO sets (set_code, name, category, release_date) VALUES
               ('NEW','Not Yet','Standalone Products','2026-11-20'),
               ('OLD','On Sale','Main Expansions','2026-03-13');
             INSERT INTO cards (card_id, name) VALUES ('c','Card');
             INSERT INTO printings (printing_id, card_id, set_code, number, variant, variant_code, finish)
               VALUES ('new1','c','NEW','1','Standard','01','Normal'),
                      ('old1','c','OLD','1','Standard','01','Normal');",
        )
        .unwrap();
    alarm(&c, "official_without_tcgcsv", "new1", "alarm");
    alarm(&c, "official_without_tcgcsv", "old1", "alarm");
    alarm(&c, "ambiguous_tcgcsv", "new1", "alarm");
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let r = check::run_report(c.conn(), now).unwrap();
    let mut open = kinds(r.open);
    open.sort();
    assert_eq!(
        open,
        [
            ("ambiguous_tcgcsv".into(), "new1".into()),
            ("official_without_tcgcsv".into(), "old1".into())
        ]
    );
    assert_eq!(r.accepted.len(), 1);
}

#[test]
fn the_accepted_table_names_each_alarm_once_with_a_reason() {
    let table = check::accepted_table();
    assert!(table.len() > 200, "{}", table.len());
    let mut seen = std::collections::BTreeSet::new();
    for (kind, key, reason) in &table {
        assert!(!kind.is_empty() && !key.is_empty(), "{kind} {key}");
        assert!(reason.len() > 20, "{kind} {key}: reason {reason:?}");
        assert!(seen.insert((kind, key)), "{kind} {key} listed twice");
    }
}
