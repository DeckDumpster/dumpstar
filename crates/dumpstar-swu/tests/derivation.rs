//! A change in what an apply derives from a source re-applies the payloads
//! already applied, from the raw already landed (sp-boex8e).

use chrono::{TimeZone, Utc};
use dumpstar_swu::Catalog;
use dumpstar_swu::apply::apply_at;
use dumpstar_swu::landing::land;
use dumpstar_swu::source::RawPayload;
use dumpstar_swu::swu::SRC_PRODUCTS;

fn images(c: &Catalog) -> i64 {
    c.conn()
        .query_row("SELECT COUNT(*) FROM sealed_images", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_bumped_derivation_reapplies_an_applied_payload_once_and_only_then() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw");
    let db = dir.path().join("shared.sqlite");
    let bytes = br#"{"success":true,"results":[{"productId":9001,"groupId":1,"name":"Spark of Rebellion Booster Box","imageUrl":"https://img.test/9001.jpg","extendedData":[]}]}"#.to_vec();
    let payload = RawPayload {
        source: SRC_PRODUCTS.into(),
        url: "https://tcgcsv.test/tcgplayer/79/1/products".into(),
        bytes: bytes.clone(),
        as_of: Some("2026-10-15".into()),
    };
    let landed = land(
        &raw,
        &payload,
        Utc.with_ymd_and_hms(2026, 10, 15, 21, 0, 0).unwrap(),
    )
    .unwrap();
    let mut c = Catalog::open_writer(&db).unwrap();

    // The earlier build derived no image rows: model it by clearing them.
    apply_at(&mut c, &bytes, &landed.provenance, 1).unwrap();
    c.conn().execute("DELETE FROM sealed_images", []).unwrap();
    assert_eq!(images(&c), 0);
    assert_eq!(
        c.reapply_landed_at(&raw, SRC_PRODUCTS, 1).unwrap(),
        0,
        "unchanged source is not re-applied"
    );
    assert_eq!(images(&c), 0);

    assert_eq!(c.reapply_landed_at(&raw, SRC_PRODUCTS, 2).unwrap(), 1);
    assert_eq!(images(&c), 1);
    assert_eq!(
        c.reapply_landed_at(&raw, SRC_PRODUCTS, 2).unwrap(),
        0,
        "once per version"
    );
}

#[test]
fn a_text_catalog_without_keywords_or_epic_actions_alarms() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    for i in 0..60 {
        c.conn()
            .execute(
                "INSERT INTO cards (card_id, name, rules_text) VALUES (?1, ?1, 'Sentinel')",
                [format!("C-{i}")],
            )
            .unwrap();
    }
    let a = dumpstar_swu::check::missing_structured_text(c.conn()).unwrap();
    assert_eq!(a.len(), 1, "{a:?}");
    c.conn()
        .execute(
            "UPDATE cards SET keywords='[{\"name\":\"Sentinel\",\"value\":null}]', epic_action='x' WHERE card_id='C-1'",
            [],
        )
        .unwrap();
    assert!(
        dumpstar_swu::check::missing_structured_text(c.conn())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn repeated_or_missing_rules_text_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    for (id, ty, text) in [
        (
            "K",
            "Leader",
            Some("Each friendly damaged unit gets +1/+0.\nEach friendly damaged unit gets +1/+0."),
        ),
        ("R", "Upgrade", None),
        ("A", "Upgrade", Some("Plot")),
        ("OK", "Upgrade", Some("Plot\nAttached unit gets +0/+2.")),
        ("V", "Unit", Some("Sentinel")),
        ("T", "Token Upgrade", None),
        (
            "M",
            "Unit",
            Some("While you control another non-<uq> unit, this unit gains Ambush."),
        ),
    ] {
        c.conn()
            .execute(
                "INSERT INTO cards (card_id, name, card_type, rules_text) VALUES (?1, ?1, ?2, ?3)",
                rusqlite::params![id, ty, text],
            )
            .unwrap();
    }
    let a = dumpstar_swu::check::suspect_rules_text(c.conn()).unwrap();
    let keys: Vec<(&str, &str)> = a
        .iter()
        .map(|r| (r.kind.as_str(), r.key.as_str()))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("rules_text_sparse", "A"),
            ("rules_text_repeated", "K"),
            ("rules_text_markup", "M"),
            ("rules_text_sparse", "R"),
        ]
    );
}

#[test]
fn a_keyword_with_a_number_in_the_text_but_null_value_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    for (id, kws, text) in [
        (
            "DEVOTION",
            "[{\"name\":\"Restore\",\"value\":null}]",
            "Attached unit gains Restore 2.",
        ),
        (
            "OK",
            "[{\"name\":\"Restore\",\"value\":2}]",
            "Attached unit gains Restore 2.",
        ),
        (
            "PLAIN",
            "[{\"name\":\"Sentinel\",\"value\":null}]",
            "Sentinel",
        ),
    ] {
        c.conn()
            .execute(
                "INSERT INTO cards (card_id, name, card_type, rules_text, keywords) VALUES (?1, ?1, 'Upgrade', ?3, ?2)",
                rusqlite::params![id, kws, text],
            )
            .unwrap();
    }
    let a = dumpstar_swu::check::keyword_number_missing(c.conn()).unwrap();
    let keys: Vec<(&str, &str)> = a
        .iter()
        .map(|r| (r.kind.as_str(), r.key.as_str()))
        .collect();
    assert_eq!(keys, vec![("keyword_number_missing", "DEVOTION")]);
}

#[test]
fn a_keyword_in_the_text_but_not_in_the_keywords_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    for (id, kws, text) in [
        ("NITE-OWL", "[]", "Support\nWhen Played: draw a card."),
        ("SNUB", "[]", "Plot\nOn Attack: deal 1 damage."),
        ("HAS", "[{\"name\":\"Support\",\"value\":null}]", "Support"),
        ("GRANTS", "[]", "Attached unit gains Sentinel."),
    ] {
        c.conn()
            .execute(
                "INSERT INTO cards (card_id, name, card_type, rules_text, keywords) VALUES (?1, ?1, 'Unit', ?3, ?2)",
                rusqlite::params![id, kws, text],
            )
            .unwrap();
    }
    let a = dumpstar_swu::check::keyword_missing_from_keywords(c.conn()).unwrap();
    let keys: Vec<(&str, &str)> = a
        .iter()
        .map(|r| (r.kind.as_str(), r.key.as_str()))
        .collect();
    assert_eq!(
        keys,
        vec![("keyword_missing", "NITE-OWL"), ("keyword_missing", "SNUB")]
    );
}

#[test]
fn reapplying_records_the_derivation_version_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    assert!(c.derivation_versions().unwrap().is_empty());
    c.reapply_landed(&dir.path().join("raw")).unwrap();
    assert!(c.derivation_versions().unwrap().contains(&(
        "official".to_string(),
        dumpstar_swu::apply::derivation_version("official")
    )));
    assert!(dumpstar_swu::apply::derivation_version("official") >= 8);
}

#[test]
fn a_mechanic_like_phrase_the_derivation_missed_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    for (id, mechanics, text) in [
        ("NOVEL", "[]", "When Played: You may spend the Force."),
        (
            "DERIVED",
            "[{\"name\":\"force\",\"role\":\"consumes\",\"optional\":true}]",
            "When Played: You may use the Force.",
        ),
        ("PLAIN", "[]", "Sentinel\nRaid 2"),
    ] {
        c.conn()
            .execute(
                "INSERT INTO cards (card_id, name, card_type, rules_text, mechanics) VALUES (?1, ?1, 'Unit', ?3, ?2)",
                rusqlite::params![id, mechanics, text],
            )
            .unwrap();
    }
    let a = dumpstar_swu::check::mechanic_unmatched(c.conn()).unwrap();
    let keys: Vec<(&str, &str)> = a
        .iter()
        .map(|r| (r.kind.as_str(), r.key.as_str()))
        .collect();
    assert_eq!(keys, vec![("mechanic_unmatched", "NOVEL")]);
}

#[test]
fn an_unmatched_phrase_files_once_and_never_fails_the_check() {
    use dumpstar_swu::filing::{Action, file};
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    c.conn()
        .execute(
            "INSERT INTO cards (card_id, name, card_type, rules_text, mechanics) VALUES ('NOVEL','NOVEL','Unit','When Played: You may spend the Force.','[]')",
            [],
        )
        .unwrap();
    let report = dumpstar_swu::check::run_report(c.conn(), chrono::Utc::now()).unwrap();
    assert!(report.open.iter().all(|a| a.kind != "mechanic_unmatched"));
    let findings = dumpstar_swu::check::mechanic_findings(c.conn()).unwrap();
    let out = dir.path().join("filings");
    let first = file(&out, &findings).unwrap().expect("one filing");
    assert_eq!(first.action, Action::Open);
    assert_eq!(first.findings.len(), 1);
    assert_eq!(first.findings[0].card_id, "NOVEL");
    assert!(first.findings[0].phrase.starts_with("the force"));
    assert!(file(&out, &findings).unwrap().is_none());
    assert_eq!(std::fs::read_dir(out.join("outbox")).unwrap().count(), 1);
    // no findings: nothing filed, state cleared for the next bead
    assert!(file(&out, &[]).unwrap().is_none());
    assert_eq!(file(&out, &findings).unwrap().unwrap().action, Action::Open);
}

#[test]
fn a_later_finding_appends_to_the_open_bead() {
    use dumpstar_swu::filing::{Action, Finding, State, plan};
    let f = |id: &str| Finding {
        card_id: id.into(),
        rules_text: "x".into(),
        phrase: "p".into(),
    };
    let (s, a) = plan(&[f("A")], &State::default());
    assert_eq!(a.unwrap().action, Action::Open);
    let (_, b) = plan(&[f("A"), f("B")], &s);
    let b = b.unwrap();
    assert_eq!(b.action, Action::Append);
    assert_eq!(b.findings, vec![f("B")]);
}
