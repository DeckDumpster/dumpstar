//! Tier 4 — differential oracle (offline, always on).
//!
//! The compiler turns a parsed query into SQL. The oracle evaluates the same
//! parsed query a second, independent way: directly, in Rust, over a small
//! in-memory table of Star Wars: Unlimited printings. The fixture database is
//! built *from* that table, so any disagreement between the two result sets is
//! a bug in the compiler (or in this evaluator, which is deliberately written
//! from the grammar's documented semantics and not from the SQL).
//!
//! The evaluator covers the card-metadata subset: names, card type, aspect,
//! arena, trait, cost/power/hp, rarity (equality, group and ordinal), set,
//! artist, rules text, year, legality, variant, finish, the printing flags,
//! `has:`, `qty`, and the owned/missing/dupe flags. Keywords with no
//! card-metadata meaning (status, condition, price, deck, modifiers, ...)
//! make [`Oracle::eval`] return `None` and the query is skipped, so the
//! coverage claim at the bottom is checked rather than assumed.
//!
//! Single queries come from a curated list; the boolean grammar (`and`, `or`,
//! `-`, grouping) is then swept by combining every pair of atoms.

use dumpstar_core::query::{Ast, KeywordRegistry, Op, parse};
use dumpstar_db::search::{self, compile};
use dumpstar_db::search_meta::{self, SearchFlag};
use dumpstar_db::{connect_user, open_shared};
use rusqlite::Connection;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// The universe
// ---------------------------------------------------------------------------

struct Card {
    id: &'static str,
    name: &'static str,
    subtitle: Option<&'static str>,
    card_type: &'static str,
    cost: Option<i64>,
    power: Option<i64>,
    hp: Option<i64>,
    aspects: &'static [&'static str],
    traits: &'static [&'static str],
    arenas: &'static [&'static str],
    rules: &'static str,
}

struct Printing {
    id: &'static str,
    card: &'static str,
    set: &'static str,
    number: &'static str,
    variant: &'static str,
    finish: &'static str,
    rarity: &'static str,
    artist: &'static str,
    /// Copies of this printing in the collection.
    owned: i64,
}

const SETS: &[(&str, &str, &str)] = &[
    ("SOR", "Spark of Rebellion", "2024-03-08"),
    ("SHD", "Shadows of the Galaxy", "2024-07-12"),
    ("TWI", "Twilight of the Republic", "2025-11-08"),
];

/// The rarities table, as `data/rarities.json` ranks it: (name, rank, group).
const RARITIES: &[(&str, i64, &str)] = &[
    ("Common", 0, "common"),
    ("Uncommon", 1, "uncommon"),
    ("Rare", 2, "rare"),
    ("Legendary", 3, "legendary"),
    ("Special", 4, "special"),
    ("Promo", 2, "promo"),
];

const CARDS: &[Card] = &[
    Card {
        id: "luke",
        name: "Luke Skywalker",
        subtitle: Some("Faithful Friend"),
        card_type: "Unit",
        cost: Some(3),
        power: Some(3),
        hp: Some(5),
        aspects: &["Vigilance", "Heroism"],
        traits: &["Force", "Rebel", "Jedi"],
        arenas: &["Ground"],
        rules: "On Attack: Heal 2 damage from a base.",
    },
    Card {
        id: "falcon",
        name: "Millennium Falcon",
        subtitle: Some("Piece of Junk"),
        card_type: "Unit",
        cost: Some(2),
        power: Some(3),
        hp: Some(3),
        aspects: &["Cunning", "Heroism"],
        traits: &["Vehicle", "Transport", "Underworld"],
        arenas: &["Space"],
        rules: "Piloting [2].",
    },
    Card {
        id: "han",
        name: "Han Solo",
        subtitle: Some("Reluctant Hero"),
        card_type: "Leader",
        cost: Some(6),
        power: Some(7),
        hp: Some(6),
        aspects: &["Aggression", "Villainy"],
        traits: &["Underworld", "Rebel"],
        arenas: &["Ground"],
        rules: "When Played: Deal 2 damage to a unit.",
    },
    Card {
        id: "vanquish",
        name: "Vanquish",
        subtitle: None,
        card_type: "Event",
        cost: Some(5),
        power: None,
        hp: None,
        aspects: &["Vigilance"],
        traits: &[],
        arenas: &[],
        rules: "Defeat a non-leader unit.",
    },
    Card {
        id: "base",
        name: "Echo Base",
        subtitle: None,
        card_type: "Base",
        cost: None,
        power: None,
        hp: Some(30),
        aspects: &["Command"],
        traits: &[],
        arenas: &[],
        rules: "Epic Action: Put a Rebel unit into play.",
    },
    Card {
        id: "vader",
        name: "Darth Vader",
        subtitle: Some("Dark Lord of the Sith"),
        card_type: "Leader",
        cost: Some(7),
        power: Some(5),
        hp: Some(7),
        aspects: &["Aggression", "Villainy"],
        traits: &["Force", "Imperial", "Sith"],
        arenas: &["Ground"],
        rules: "Action: Deal 1 damage to a unit.",
    },
];

const PRINTINGS: &[Printing] = &[
    Printing {
        id: "luke-std",
        card: "luke",
        set: "SOR",
        number: "005",
        variant: "Standard",
        finish: "Normal",
        rarity: "Legendary",
        artist: "Borja Pindado",
        owned: 2,
    },
    Printing {
        id: "luke-foil",
        card: "luke",
        set: "SOR",
        number: "005",
        variant: "Standard",
        finish: "Foil",
        rarity: "Legendary",
        artist: "Borja Pindado",
        owned: 1,
    },
    Printing {
        id: "luke-hyper",
        card: "luke",
        set: "SOR",
        number: "400",
        variant: "Hyperspace",
        finish: "Normal",
        rarity: "Legendary",
        artist: "Borja Pindado",
        owned: 0,
    },
    Printing {
        id: "falcon-std",
        card: "falcon",
        set: "SHD",
        number: "252",
        variant: "Standard",
        finish: "Normal",
        rarity: "Common",
        artist: "Ralph McQuarrie",
        owned: 1,
    },
    Printing {
        id: "falcon-showcase",
        card: "falcon",
        set: "SHD",
        number: "520",
        variant: "Showcase",
        finish: "Normal",
        rarity: "Special",
        artist: "Ralph McQuarrie",
        owned: 0,
    },
    Printing {
        id: "han-std",
        card: "han",
        set: "SOR",
        number: "010",
        variant: "Standard",
        finish: "Normal",
        rarity: "Rare",
        artist: "Ryan Grayson",
        owned: 0,
    },
    Printing {
        id: "han-promo",
        card: "han",
        set: "TWI",
        number: "P01",
        variant: "Promo",
        finish: "Foil",
        rarity: "Promo",
        artist: "",
        owned: 3,
    },
    Printing {
        id: "vanquish-std",
        card: "vanquish",
        set: "SOR",
        number: "193",
        variant: "Standard",
        finish: "Normal",
        rarity: "Uncommon",
        artist: "Kamil Murzyn",
        owned: 0,
    },
    Printing {
        id: "base-std",
        card: "base",
        set: "SOR",
        number: "020",
        variant: "Standard",
        finish: "Normal",
        rarity: "Common",
        artist: "Eric Robinson",
        owned: 0,
    },
    Printing {
        id: "vader-std",
        card: "vader",
        set: "SHD",
        number: "001",
        variant: "Standard",
        finish: "Normal",
        rarity: "Rare",
        artist: "Ryan Grayson",
        owned: 0,
    },
];

/// (format, card, status). A card with no row is legal in every format.
const LEGALITY: &[(&str, &str, &str)] = &[
    ("premier", "vader", "banned"),
    ("premier", "falcon", "not_legal"),
    ("twin_suns", "luke", "banned"),
    ("twin_suns", "han", "restricted"),
];

fn fixture() -> (
    tempfile::TempDir,
    Connection,
    KeywordRegistry,
    Vec<SearchFlag>,
) {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared.sqlite");
    {
        let mut c = open_shared(&shared).unwrap();
        search_meta::reconcile(&mut c).unwrap();
        for (i, (code, name, date)) in SETS.iter().enumerate() {
            c.execute(
                "INSERT INTO sets (set_code,name,release_date,sort_order) VALUES (?1,?2,?3,?4)",
                rusqlite::params![code, name, date, i as i64],
            )
            .unwrap();
        }
        for card in CARDS {
            let json = |xs: &[&str]| serde_json::to_string(xs).unwrap();
            c.execute(
                "INSERT INTO cards (card_id,name,subtitle,card_type,cost,power,hp,aspects,traits,
                    arenas,rules_text) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                rusqlite::params![
                    card.id,
                    card.name,
                    card.subtitle,
                    card.card_type,
                    card.cost,
                    card.power,
                    card.hp,
                    json(card.aspects),
                    json(card.traits),
                    json(card.arenas),
                    card.rules
                ],
            )
            .unwrap();
        }
        for p in PRINTINGS {
            c.execute(
                "INSERT INTO printings (printing_id,card_id,set_code,number,variant,finish,
                    rarity,artist) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    p.id, p.card, p.set, p.number, p.variant, p.finish, p.rarity, p.artist
                ],
            )
            .unwrap();
        }
        for format in ["premier", "twin_suns"] {
            c.execute(
                "INSERT INTO formats (format_id,name) VALUES (?1,?1)",
                [format],
            )
            .unwrap();
        }
        for (format, card, status) in LEGALITY {
            c.execute(
                "INSERT INTO card_legality (format_id,card_id,status) VALUES (?1,?2,?3)",
                rusqlite::params![format, card, status],
            )
            .unwrap();
        }
    }
    let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
    for p in PRINTINGS {
        for _ in 0..p.owned {
            conn.execute(
                "INSERT INTO collection (printing_id,condition,language,acquired_at,source,status)
                 VALUES (?1,'Near Mint','English','2024-01-01','manual_id','owned')",
                [p.id],
            )
            .unwrap();
        }
    }
    let registry = search_meta::load_registry(&conn).unwrap();
    let flags = search_meta::load_flags(&conn).unwrap();
    (dir, conn, registry, flags)
}

// ---------------------------------------------------------------------------
// The oracle
// ---------------------------------------------------------------------------

struct Oracle;

fn ci_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn ci_contains(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn cmp_num(op: Op, lhs: Option<i64>, rhs: f64) -> bool {
    let Some(l) = lhs else { return false };
    let l = l as f64;
    match op {
        Op::Contains | Op::Eq => l == rhs,
        Op::Ne => l != rhs,
        Op::Lt => l < rhs,
        Op::Gt => l > rhs,
        Op::Le => l <= rhs,
        Op::Ge => l >= rhs,
    }
}

fn rank_of(name: &str) -> Option<i64> {
    RARITIES.iter().find(|r| r.0 == name).map(|r| r.1)
}

fn set_of(p: &Printing) -> (&'static str, &'static str, &'static str) {
    *SETS.iter().find(|s| s.0 == p.set).unwrap()
}

fn card_of(p: &Printing) -> &'static Card {
    CARDS.iter().find(|c| c.id == p.card).unwrap()
}

fn member(xs: &[&str], v: &str, op: Op) -> bool {
    let has = xs.iter().any(|x| ci_eq(x, v));
    if matches!(op, Op::Ne) { !has } else { has }
}

impl Oracle {
    /// Does `p` match `ast`? `None` when the query uses a keyword the oracle
    /// does not model.
    fn eval(ast: &Ast, p: &Printing) -> Option<bool> {
        let card = card_of(p);
        Some(match ast {
            Ast::And(cs) => {
                let mut all = true;
                for c in cs {
                    all &= Self::eval(c, p)?;
                }
                all
            }
            Ast::Or(cs) => {
                let mut any = false;
                for c in cs {
                    any |= Self::eval(c, p)?;
                }
                any
            }
            Ast::Not(inner) => !Self::eval(inner, p)?,
            Ast::NameSearch(t) => ci_contains(card.name, t),
            Ast::ExactName(n) => ci_eq(card.name, n),
            Ast::Comparison { keyword, op, value } => {
                let (op, v) = (*op, value.as_str());
                match keyword.as_str() {
                    "name" => match op {
                        Op::Eq => ci_eq(card.name, v),
                        Op::Ne => !ci_eq(card.name, v),
                        _ => ci_contains(card.name, v),
                    },
                    "card_type" => match op {
                        Op::Ne => !ci_eq(card.card_type, v),
                        _ => ci_eq(card.card_type, v),
                    },
                    "finish" => match op {
                        Op::Ne => !ci_eq(p.finish, v),
                        _ => ci_eq(p.finish, v),
                    },
                    "aspect" => member(card.aspects, v, op),
                    "arena" => member(card.arenas, v, op),
                    "trait" => member(card.traits, v, op),
                    "cost" => cmp_num(op, card.cost, v.parse().ok()?),
                    "power" => cmp_num(op, card.power, v.parse().ok()?),
                    "hp" => cmp_num(op, card.hp, v.parse().ok()?),
                    "year" => {
                        let year: i64 = set_of(p).2[..4].parse().unwrap();
                        cmp_num(op, Some(year), v.parse().ok()?)
                    }
                    "artist" => ci_contains(p.artist, v),
                    "oracle" => ci_contains(card.rules, v),
                    "variant" => {
                        let m = ci_contains(p.variant, v);
                        if matches!(op, Op::Ne) { !m } else { m }
                    }
                    "rarity" => {
                        let eq = ci_eq(p.rarity, v)
                            || RARITIES.iter().any(|r| r.0 == p.rarity && ci_eq(r.2, v));
                        match op {
                            Op::Contains | Op::Eq => eq,
                            Op::Ne => !eq,
                            _ => {
                                // The right-hand side is a rarity *name*; an
                                // unknown one ranks NULL and matches nothing.
                                let rhs = RARITIES.iter().find(|r| ci_eq(r.0, v))?.1;
                                cmp_num(op, rank_of(p.rarity), rhs as f64)
                            }
                        }
                    }
                    "set" => {
                        let (code, name, _) = set_of(p);
                        match op {
                            Op::Eq => ci_eq(code, v),
                            Op::Ne => !ci_eq(code, v),
                            _ => ci_eq(code, v) || ci_contains(name, v),
                        }
                    }
                    "legality" | "banned" => {
                        let banned = keyword == "banned";
                        if !LEGALITY.iter().any(|l| ci_eq(l.0, v)) {
                            // An unknown format matches nothing, either way.
                            return Some(false);
                        }
                        let status = LEGALITY
                            .iter()
                            .find(|l| ci_eq(l.0, v) && l.1 == card.id)
                            .map(|l| l.2);
                        if banned {
                            status == Some("banned")
                        } else {
                            !matches!(status, Some("banned" | "not_legal"))
                        }
                    }
                    "is_flag" => match v.to_ascii_lowercase().as_str() {
                        "foil" => p.finish == "Foil",
                        "nonfoil" => p.finish == "Normal",
                        "standard" => ci_contains(p.variant, "standard"),
                        "hyperspace" => ci_contains(p.variant, "hyperspace"),
                        "showcase" => ci_contains(p.variant, "showcase"),
                        "promo" => ci_contains(p.variant, "promo"),
                        "dupe" => p.owned >= 2,
                        "owned" => p.owned > 0,
                        "missing" | "unowned" => p.owned == 0,
                        _ => return None,
                    },
                    "has_flag" => match v.to_ascii_lowercase().as_str() {
                        "text" => !card.rules.is_empty(),
                        "subtitle" => card.subtitle.is_some_and(|s| !s.is_empty()),
                        "artist" => !p.artist.is_empty(),
                        _ => return None,
                    },
                    "qty" => cmp_num(op, Some(p.owned), v.parse().ok()?),
                    _ => return None,
                }
            }
        })
    }
}

/// Run `query` catalog-wide through the compiler and compare with the oracle.
/// Returns `false` (and compares nothing) when the oracle does not model it.
fn agrees(conn: &Connection, reg: &KeywordRegistry, flags: &[SearchFlag], query: &str) -> bool {
    let ast = parse(query, reg).unwrap_or_else(|e| panic!("{query:?} must parse: {e}"));
    let mut expected = BTreeSet::new();
    for p in PRINTINGS {
        match Oracle::eval(&ast, p) {
            Some(true) => {
                expected.insert(p.id.to_string());
            }
            Some(false) => {}
            None => return false,
        }
    }
    let mut compiled = compile(&ast, flags);
    assert!(compiled.is_supported(), "{query:?} compiled to a sentinel");
    compiled.set_catalog_wide(true);
    let got: BTreeSet<String> = search::search(conn, &compiled)
        .unwrap_or_else(|e| panic!("{query:?} failed to execute: {e:?}"))
        .into_iter()
        .map(|r| r.printing_id)
        .collect();
    assert_eq!(
        got,
        expected,
        "{query:?}: SQL result (left) disagrees with the oracle (right)\n  where: {}",
        compiled.where_sql()
    );
    true
}

// ---------------------------------------------------------------------------
// Atoms: one or more per supported card-metadata keyword
// ---------------------------------------------------------------------------

const ATOMS: &[&str] = &[
    "luke",
    "han",
    "solo",
    "name:falcon",
    "name=Vanquish",
    "name!=Vanquish",
    "!\"Darth Vader\"",
    "t:unit",
    "type:leader",
    "t=event",
    "t!=unit",
    "t:base",
    "a:vigilance",
    "aspect:heroism",
    "a=villainy",
    "a!=aggression",
    "arena:ground",
    "arena:space",
    "arena!=ground",
    "trait:rebel",
    "trait:force",
    "trait!=underworld",
    "cost:3",
    "c<=3",
    "c>=6",
    "cost>4",
    "c!=2",
    "power:3",
    "p>=5",
    "p<3",
    "p!=3",
    "hp:5",
    "hp>=6",
    "hp<=3",
    "hp>10",
    "hp!=30",
    "r:common",
    "r:legendary",
    "r:promo",
    "r=rare",
    "r!=common",
    "r>=rare",
    "r<rare",
    "r>uncommon",
    "r<=rare",
    "s:sor",
    "s:shadows",
    "e:twi",
    "s=shd",
    "s!=sor",
    "artist:pindado",
    "artist:\"Ralph McQuarrie\"",
    "o:damage",
    "text:unit",
    "o:\"on attack\"",
    "year:2024",
    "year>=2025",
    "year<2025",
    "legal:premier",
    "legal:twin_suns",
    "format:premier",
    "banned:premier",
    "banned:twin_suns",
    "legal:nonexistent",
    "v:standard",
    "variant:hyperspace",
    "v!=standard",
    "v:promo",
    "f:foil",
    "finish:normal",
    "f!=foil",
    "is:foil",
    "is:nonfoil",
    "is:standard",
    "is:hyperspace",
    "is:showcase",
    "is:promo",
    "is:dupe",
    "is:owned",
    "is:missing",
    "is:unowned",
    "has:text",
    "has:subtitle",
    "has:artist",
    "qty:0",
    "qty>=1",
    "qty>1",
    "count<3",
];

#[test]
fn every_atom_agrees_with_the_oracle() {
    let (_d, conn, reg, flags) = fixture();
    let mut skipped = Vec::new();
    for q in ATOMS {
        if !agrees(&conn, &reg, &flags, q) {
            skipped.push(*q);
        }
    }
    assert!(
        skipped.is_empty(),
        "every atom is meant to be modelled by the oracle, but these were not: {skipped:?}"
    );
}

#[test]
fn boolean_combinations_agree_with_the_oracle() {
    let (_d, conn, reg, flags) = fixture();
    let mut checked = 0;
    // Every pair, as AND, OR, and negated-AND; plus a grouped form. Each pair of atoms is taken exactly once.
    for (i, a) in ATOMS.iter().enumerate() {
        for b in &ATOMS[i + 1..] {
            for q in [
                format!("{a} {b}"),
                format!("{a} or {b}"),
                format!("-({a} {b})"),
                format!("({a} or {b}) -({a})"),
            ] {
                assert!(agrees(&conn, &reg, &flags, &q), "{q:?} not modelled");
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000, "sweep shrank to {checked} queries");
}

#[test]
fn unmodelled_keywords_are_skipped_not_guessed() {
    // The coverage claim: collection and pricing keywords are not card
    // metadata, so the oracle declines them rather than answering.
    let (_d, conn, reg, flags) = fixture();
    for q in [
        "status:owned",
        "condition:nm",
        "price>5",
        "deck:*",
        "is:graded",
        "tag:favorite",
    ] {
        assert!(!agrees(&conn, &reg, &flags, q), "{q:?} should be skipped");
    }
}

#[test]
fn the_oracle_is_not_vacuous() {
    // Positive control: a query that matches some printings and not others, so
    // the equality in `agrees` compares two non-trivial sets rather than two
    // empty ones (or two full ones).
    let (_d, conn, reg, flags) = fixture();
    let ast = parse("a:vigilance", &reg).unwrap();
    let expected: BTreeSet<&str> = PRINTINGS
        .iter()
        .filter(|p| Oracle::eval(&ast, p) == Some(true))
        .map(|p| p.id)
        .collect();
    // Luke's three printings and Vanquish.
    assert_eq!(
        expected,
        BTreeSet::from(["luke-std", "luke-foil", "luke-hyper", "vanquish-std"])
    );
    assert!(agrees(&conn, &reg, &flags, "a:vigilance"));
}
