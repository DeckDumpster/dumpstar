//! Format rules as data, and the deck checker run against them: one passing
//! and one failing deck per format for each rule (size, copy cap, leader and
//! base counts, banned card). The rules come out of the catalog tables, so a
//! wrong row in `data/formats.json` fails here, and every citation in that file
//! is checked against the retrieved Comprehensive Rules text.

use std::collections::HashMap;
use std::io::Read;

use dumpstar_catalog::Catalog;
use dumpstar_catalog::formats::builtin_formats;
use dumpstar_core::deck::{DeckEntry, Violation, Zone, check_deck};
use rusqlite::params;

const BANNED_IN: &[&str] = &["premier", "eternal", "twin_suns", "limited"];

fn catalog() -> (tempfile::TempDir, Catalog) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    c.seed_formats().unwrap();
    let conn = c.conn();
    conn.execute(
        "INSERT INTO sets (set_code, name) VALUES ('SOR','Spark of Rebellion')",
        [],
    )
    .unwrap();
    let cards: &[(&str, &str, &str)] = &[
        ("L_HERO", "Leader", r#"["Heroism","Command"]"#),
        ("L_HERO2", "Leader", r#"["Heroism","Cunning"]"#),
        ("L_VILL", "Leader", r#"["Villainy","Command"]"#),
        ("L_NEUT", "Leader", r#"["Vigilance"]"#),
        ("BASE", "Base", r#"["Command"]"#),
        ("BASE2", "Base", r#"["Cunning"]"#),
        ("BANNED", "Unit", r#"["Command"]"#),
    ];
    for (id, ty, asp) in cards {
        conn.execute(
            "INSERT INTO cards (card_id, name, card_type, aspects) VALUES (?1,?1,?2,?3)",
            params![id, ty, asp],
        )
        .unwrap();
    }
    for n in 0..60 {
        conn.execute(
            "INSERT INTO cards (card_id, name, card_type) VALUES (?1,?1,'Unit')",
            params![format!("U{n:02}")],
        )
        .unwrap();
    }
    for f in BANNED_IN {
        conn.execute(
            "INSERT INTO card_legality (format_id, card_id, status) VALUES (?1,'BANNED','banned')",
            [f],
        )
        .unwrap();
    }
    (dir, c)
}

fn e(card: &str, qty: u32, zone: Zone) -> DeckEntry {
    DeckEntry {
        card_id: card.into(),
        qty,
        zone,
    }
}

/// `n` distinct units U00.., `per` copies each, in the main deck.
fn units(n: u32, per: u32) -> Vec<DeckEntry> {
    (0..n)
        .map(|i| e(&format!("U{i:02}"), per, Zone::Main))
        .collect()
}

fn deck(parts: &[Vec<DeckEntry>]) -> Vec<DeckEntry> {
    parts.iter().flatten().cloned().collect()
}

struct Case {
    name: &'static str,
    format: &'static str,
    deck: Vec<DeckEntry>,
    /// Empty = must be legal; otherwise exactly these violations.
    expect: Vec<Violation>,
}

fn cases() -> Vec<Case> {
    let one = |c: &str| vec![e(c, 1, Zone::Main)];
    let ids = |d: &[DeckEntry]| d.iter().map(|x| x.card_id.clone()).collect::<Vec<_>>();
    let _ = ids;
    let mut v = Vec::new();

    // --- Premier and Eternal share section 9.2; both are run. ---
    for f in ["premier", "eternal"] {
        let base = || deck(&[one("L_HERO"), one("BASE"), units(50, 1)]);
        v.push(Case {
            name: "min size ok",
            format: f,
            deck: base(),
            expect: vec![],
        });
        v.push(Case {
            name: "min size: 49",
            format: f,
            deck: deck(&[one("L_HERO"), one("BASE"), units(49, 1)]),
            expect: vec![Violation::TooFewCards { min: 50, found: 49 }],
        });
        v.push(Case {
            name: "copy cap ok at 3",
            format: f,
            deck: deck(&[one("L_HERO"), one("BASE"), units(17, 3)]),
            expect: vec![],
        });
        v.push(Case {
            name: "copy cap: 4 copies",
            format: f,
            deck: deck(&[
                one("L_HERO"),
                one("BASE"),
                units(16, 3),
                vec![e("U16", 4, Zone::Main)],
            ]),
            expect: vec![Violation::TooManyCopies {
                card_id: "U16".into(),
                max: 3,
                found: 4,
            }],
        });
        v.push(Case {
            name: "copy cap counts the sideboard too",
            format: f,
            deck: deck(&[
                one("L_HERO"),
                one("BASE"),
                units(17, 3),
                vec![e("U00", 1, Zone::Sideboard)],
            ]),
            expect: vec![Violation::TooManyCopies {
                card_id: "U00".into(),
                max: 3,
                found: 4,
            }],
        });
        v.push(Case {
            name: "sideboard of 10 ok",
            format: f,
            deck: deck(&[
                base(),
                (10..20)
                    .map(|i| e(&format!("U{i:02}"), 1, Zone::Sideboard))
                    .collect(),
            ]),
            expect: vec![],
        });
        v.push(Case {
            name: "sideboard of 11",
            format: f,
            deck: deck(&[
                base(),
                (10..21)
                    .map(|i| e(&format!("U{i:02}"), 1, Zone::Sideboard))
                    .collect(),
            ]),
            expect: vec![Violation::SideboardTooLarge { max: 10, found: 11 }],
        });
        v.push(Case {
            name: "no leader",
            format: f,
            deck: deck(&[one("BASE"), units(50, 1)]),
            expect: vec![Violation::WrongLeaderCount {
                required: 1,
                found: 0,
            }],
        });
        v.push(Case {
            name: "two leaders",
            format: f,
            deck: deck(&[one("L_HERO"), one("L_VILL"), one("BASE"), units(50, 1)]),
            expect: vec![Violation::WrongLeaderCount {
                required: 1,
                found: 2,
            }],
        });
        v.push(Case {
            name: "no base",
            format: f,
            deck: deck(&[one("L_HERO"), units(50, 1)]),
            expect: vec![Violation::WrongBaseCount {
                required: 1,
                found: 0,
            }],
        });
        v.push(Case {
            name: "two bases",
            format: f,
            deck: deck(&[one("L_HERO"), one("BASE"), one("BASE2"), units(50, 1)]),
            expect: vec![Violation::WrongBaseCount {
                required: 1,
                found: 2,
            }],
        });
        v.push(Case {
            name: "banned card",
            format: f,
            deck: deck(&[base(), one("BANNED")]),
            expect: vec![Violation::CardNotLegal {
                card_id: "BANNED".into(),
                status: dumpstar_core::deck::Legality::Banned,
            }],
        });
        v.push(Case {
            name: "heroism and villainy leaders are not restricted here",
            format: f,
            deck: deck(&[one("L_HERO"), one("BASE"), units(50, 1)]),
            expect: vec![],
        });
    }

    // --- Twin Suns, section 12. ---
    let ts =
        |units: Vec<DeckEntry>, l1: &str, l2: &str| deck(&[one(l1), one(l2), one("BASE"), units]);
    v.push(Case {
        name: "two leaders, 80, singleton ok",
        format: "twin_suns",
        deck: ts(
            units(60, 1)
                .into_iter()
                .chain((0..20).map(|i| e(&format!("X{i}"), 1, Zone::Main)))
                .collect(),
            "L_HERO",
            "L_HERO2",
        ),
        expect: vec![],
    });
    v.push(Case {
        name: "79 cards",
        format: "twin_suns",
        deck: ts(
            units(60, 1)
                .into_iter()
                .chain((0..19).map(|i| e(&format!("X{i}"), 1, Zone::Main)))
                .collect(),
            "L_HERO",
            "L_HERO2",
        ),
        expect: vec![Violation::TooFewCards { min: 80, found: 79 }],
    });
    v.push(Case {
        name: "one leader",
        format: "twin_suns",
        deck: deck(&[
            one("L_HERO"),
            one("BASE"),
            units(60, 1),
            (0..20)
                .map(|i| e(&format!("X{i}"), 1, Zone::Main))
                .collect(),
        ]),
        expect: vec![Violation::WrongLeaderCount {
            required: 2,
            found: 1,
        }],
    });
    v.push(Case {
        name: "heroism with villainy leaders",
        format: "twin_suns",
        deck: ts(
            units(60, 1)
                .into_iter()
                .chain((0..20).map(|i| e(&format!("X{i}"), 1, Zone::Main)))
                .collect(),
            "L_HERO",
            "L_VILL",
        ),
        expect: vec![Violation::LeaderAspectConflict {
            aspects: vec!["Heroism".into(), "Villainy".into()],
        }],
    });
    v.push(Case {
        name: "singleton: two copies",
        format: "twin_suns",
        deck: ts(
            units(59, 1)
                .into_iter()
                .chain((0..19).map(|i| e(&format!("X{i}"), 1, Zone::Main)))
                .chain([e("U00", 1, Zone::Main), e("U59", 2, Zone::Main)])
                .collect(),
            "L_HERO",
            "L_HERO2",
        ),
        expect: vec![
            Violation::TooManyCopies {
                card_id: "U00".into(),
                max: 1,
                found: 2,
            },
            Violation::TooManyCopies {
                card_id: "U59".into(),
                max: 1,
                found: 2,
            },
        ],
    });
    v.push(Case {
        name: "same leader twice",
        format: "twin_suns",
        deck: ts(
            units(60, 1)
                .into_iter()
                .chain((0..20).map(|i| e(&format!("X{i}"), 1, Zone::Main)))
                .collect(),
            "L_HERO",
            "L_HERO",
        ),
        expect: vec![Violation::DuplicateLeader {
            card_id: "L_HERO".into(),
        }],
    });
    v.push(Case {
        name: "banned card",
        format: "twin_suns",
        deck: ts(
            units(60, 1)
                .into_iter()
                .chain((0..19).map(|i| e(&format!("X{i}"), 1, Zone::Main)))
                .chain(one("BANNED"))
                .collect(),
            "L_HERO",
            "L_HERO2",
        ),
        expect: vec![Violation::CardNotLegal {
            card_id: "BANNED".into(),
            status: dumpstar_core::deck::Legality::Banned,
        }],
    });

    // --- Limited, section 10. ---
    v.push(Case {
        name: "30 cards ok, no cap",
        format: "limited",
        deck: deck(&[one("L_NEUT"), one("BASE"), units(10, 3)]),
        expect: vec![],
    });
    v.push(Case {
        name: "many copies allowed",
        format: "limited",
        deck: deck(&[one("L_NEUT"), one("BASE"), vec![e("U00", 30, Zone::Main)]]),
        expect: vec![],
    });
    v.push(Case {
        name: "29 cards",
        format: "limited",
        deck: deck(&[one("L_NEUT"), one("BASE"), units(29, 1)]),
        expect: vec![Violation::TooFewCards { min: 30, found: 29 }],
    });
    v.push(Case {
        name: "no leader",
        format: "limited",
        deck: deck(&[one("BASE"), units(30, 1)]),
        expect: vec![Violation::WrongLeaderCount {
            required: 1,
            found: 0,
        }],
    });
    v.push(Case {
        name: "no base",
        format: "limited",
        deck: deck(&[one("L_NEUT"), units(30, 1)]),
        expect: vec![Violation::WrongBaseCount {
            required: 1,
            found: 0,
        }],
    });
    v.push(Case {
        name: "banned card",
        format: "limited",
        deck: deck(&[one("L_NEUT"), one("BASE"), units(30, 1), one("BANNED")]),
        expect: vec![Violation::CardNotLegal {
            card_id: "BANNED".into(),
            status: dumpstar_core::deck::Legality::Banned,
        }],
    });
    v
}

#[test]
fn every_format_case_matches() {
    let (_d, c) = catalog();
    for case in cases() {
        // X0..X19 are extra units only twin_suns cases use; add on demand.
        for entry in &case.deck {
            if entry.card_id.starts_with('X') {
                c.conn()
                    .execute(
                        "INSERT OR IGNORE INTO cards (card_id, name, card_type) VALUES (?1,?1,'Unit')",
                        [&entry.card_id],
                    )
                    .unwrap();
            }
        }
        let rules = c.format_rules(case.format).unwrap().expect("format exists");
        let ids: Vec<&str> = case.deck.iter().map(|d| d.card_id.as_str()).collect();
        let facts = c.card_facts(case.format, &ids).unwrap();
        let report = check_deck(&rules, &case.deck, &facts);
        assert_eq!(
            report.violations, case.expect,
            "{} / {}: {:?}",
            case.format, case.name, report.counts
        );
        assert_eq!(report.legal, case.expect.is_empty());
    }
}

#[test]
fn printings_do_not_multiply_a_card() {
    // Entries are card_ids: the same card listed twice (as two lines) is one card.
    let (_d, c) = catalog();
    let rules = c.format_rules("premier").unwrap().unwrap();
    let d = deck(&[
        vec![e("L_HERO", 1, Zone::Main), e("BASE", 1, Zone::Main)],
        units(16, 3),
        vec![e("U16", 2, Zone::Main), e("U16", 2, Zone::Main)],
    ]);
    let ids: Vec<&str> = d.iter().map(|x| x.card_id.as_str()).collect();
    let r = check_deck(&rules, &d, &c.card_facts("premier", &ids).unwrap());
    assert_eq!(
        r.violations,
        vec![Violation::TooManyCopies {
            card_id: "U16".into(),
            max: 3,
            found: 4
        }]
    );
}

#[test]
fn unknown_card_and_missing_format_are_reported_not_assumed() {
    let (_d, c) = catalog();
    assert!(c.format_rules("no_such_format").unwrap().is_none());
    let rules = c.format_rules("limited").unwrap().unwrap();
    let r = check_deck(&rules, &[e("GHOST", 1, Zone::Main)], &HashMap::new());
    assert!(r.violations.contains(&Violation::UnknownCard {
        card_id: "GHOST".into()
    }));
}

#[test]
fn seeding_is_idempotent() {
    let (_d, mut c) = catalog();
    assert_eq!(c.seed_formats().unwrap(), 0);
}

fn rules_text() -> String {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/swu/rules/comprehensive-rules-v8.0.extracted.txt.gz"
    );
    let mut s = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(p).unwrap())
        .read_to_string(&mut s)
        .unwrap();
    // Collapse the extractor's line breaks so phrases can be matched.
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_cited_rules_say_what_the_data_says() {
    let t = rules_text();
    let must = |s: &str| assert!(t.contains(s), "rules text lacks {s:?}");
    // Premier / Eternal, 9.2
    must("Exactly 1 leader");
    must("Exactly 1 base");
    must("A minimum of 50 other cards");
    must("cannot include more than 3 copies of any unit, event, or upgrade");
    must("sideboard of up to 10 cards");
    // Limited, 10
    must("A minimum of 30 other cards");
    must("There is no limit to the number of copies of a single card");
    // Twin Suns, 12
    must("Exactly 2 different leaders");
    must("At least 80 other cards");
    must("each deck can only contain one copy of each card");
    // Deck size
    must("The minimum number of cards in a deck varies by format");

    for f in builtin_formats() {
        let src = &f.rules["source"];
        assert!(
            src.starts_with("Comprehensive Rules v8.0 section "),
            "{}: {src}",
            f.format_id
        );
    }
}

#[test]
fn every_format_rule_set_parses() {
    let (_d, c) = catalog();
    for f in builtin_formats() {
        assert!(
            c.format_rules(&f.format_id).unwrap().is_some(),
            "{}",
            f.format_id
        );
    }
}
