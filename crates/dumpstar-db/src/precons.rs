//! Preconstructed decklists, a catalog source authored in git (`data/precons/`).
//!
//! The files are compiled in by `build.rs`, so a new deck is a new file and no
//! code edit. Every convergence reconciles them into `precon_decks`,
//! `precon_cards` (the deck) and `precon_extras` (what else is in the box): each
//! line names the Normal printing that carries a set and collector number, and a
//! line can start resolving when a derive brings the printing in, which is why
//! this runs every time rather than once per revision.
//!
//! The number alone names the printing: a number above the set's size is a
//! Hyperspace printing. Where variants share a number the Standard one is meant;
//! where several share it and none is Standard (promo sets), the line is
//! ambiguous and does not resolve.
//!
//! A defect never fails the build. It raises a `precon_unresolved` alarm keyed
//! `<file>:<line>` (1-based card line; `0` for the file as a whole) or
//! `<file>:extras:<line>`, which `catalog-check` reports, and the deck is stored
//! `offered = 0`.
//!
//! [`list`] and [`add`] are the tenant's side: an offered deck becomes, in one
//! transaction, a batch, a new copy of every card in the box and a deck that
//! reserves exactly the copies of its own cards.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use dumpstar_swu::crosswalk::norm_number;
use dumpstar_swu::record::AlarmRecord;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::batches::{self, NewBatch};
use crate::collection::{self, NewCopy};
use crate::convergence::hash_inputs;
use crate::decks::{self, NewDeck, Pick, SetLines};
use crate::error::{DbError, Result};

include!(concat!(env!("OUT_DIR"), "/precons.rs"));

/// This file, hashed into [`precons_rev`]: a changed reader is a changed build
/// of the same decklists.
pub(crate) const READER: &str = include_str!("precons.rs");

const ALARM_KIND: &str = "precon_unresolved";
const REV_KEY: &str = "precons.rev";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Spotlight,
    Starter,
    TwinSuns,
    Intro,
}

impl Kind {
    fn as_str(&self) -> &'static str {
        match self {
            Kind::Spotlight => "spotlight",
            Kind::Starter => "starter",
            Kind::TwinSuns => "twin-suns",
            Kind::Intro => "intro",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Zone {
    #[default]
    Main,
    Sideboard,
}

impl Zone {
    fn as_str(self) -> &'static str {
        match self {
            Zone::Main => "main",
            Zone::Sideboard => "sideboard",
        }
    }
}

/// One `data/precons/*.json` file. Fields not named here (provenance notes such
/// as `corrected`) are ignored.
#[derive(Debug, Deserialize)]
struct DeckFile {
    product_id: i64,
    name: String,
    set_code: String,
    kind: Kind,
    source: String,
    /// The member's name when two files share a `product_id`.
    deck: Option<String>,
    /// The number of cards the deck holds, checked against its lines.
    total: i64,
    cards: Vec<Line>,
    /// Cards that come in the product and are not in the deck. Their `zone` is
    /// not stored.
    #[serde(default)]
    extras: Vec<Line>,
}

#[derive(Debug, Deserialize)]
struct Line {
    set: String,
    number: String,
    qty: i64,
    #[serde(default)]
    zone: Zone,
}

/// What one reconciliation wrote.
#[derive(Debug, Default)]
pub struct Report {
    pub decks: usize,
    pub offered: usize,
    pub alarms: Vec<AlarmRecord>,
}

/// The revision of `files` as this build reads them.
fn precons_rev(files: &[(&str, &str)]) -> String {
    let mut h = Sha256::new();
    hash_inputs(&mut h, &[("precons.rs", READER)]);
    hash_inputs(&mut h, files);
    format!("{:x}", h.finalize())
}

/// The Normal printing a set and collector number name.
struct Numbered {
    printing_id: String,
    card_id: String,
    variant: String,
    /// The number names no one printing: variants share it, none Standard.
    ambiguous: bool,
}

/// By `(set_code, normalised number)`.
type Printings = HashMap<(String, String), Numbered>;

fn normal_printings(tx: &Transaction) -> Result<Printings> {
    // Standard first, so it is the one kept where variants share a number.
    let mut stmt = tx.prepare(
        "SELECT set_code, number, printing_id, card_id, variant FROM printings \
          WHERE finish = 'Normal' \
          ORDER BY variant <> 'Standard', variant_code, printing_id",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Printings::new();
    while let Some(r) = rows.next()? {
        let number: String = r.get(1)?;
        let variant: String = r.get(4)?;
        match out.entry((r.get(0)?, norm_number(&number))) {
            Entry::Vacant(slot) => {
                slot.insert(Numbered {
                    printing_id: r.get(2)?,
                    card_id: r.get(3)?,
                    variant,
                    ambiguous: false,
                });
            }
            Entry::Occupied(mut slot) => {
                let kept = slot.get_mut();
                kept.ambiguous |= kept.variant != "Standard" && kept.variant != variant;
            }
        }
    }
    Ok(out)
}

/// One list of a deck file, resolved against the catalog.
struct Resolved<'a> {
    /// `(1-based line, line, printing)` for each line that resolved.
    hits: Vec<(usize, &'a Line, &'a Numbered)>,
    /// The cards the lines with a positive qty add up to.
    count: i64,
}

/// Resolve `lines`, reporting each that does not to `defect` by its 1-based
/// position in the list.
fn resolve<'a>(
    lines: &'a [Line],
    printings: &'a Printings,
    mut defect: impl FnMut(usize, String),
) -> Resolved<'a> {
    let mut out = Resolved {
        hits: Vec::new(),
        count: 0,
    };
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if line.qty < 1 {
            defect(n, format!("qty {} is not a positive count", line.qty));
            continue;
        }
        out.count = out.count.saturating_add(line.qty);
        match printings.get(&(line.set.clone(), norm_number(&line.number))) {
            Some(hit) if !hit.ambiguous => out.hits.push((n, line, hit)),
            Some(_) => defect(
                n,
                format!(
                    "several variants carry {} {} and none is Standard",
                    line.set, line.number
                ),
            ),
            None => defect(
                n,
                format!("no Normal printing {} {}", line.set, line.number),
            ),
        }
    }
    out
}

/// Make `precon_decks`, `precon_cards`, `precon_extras`, the
/// `precon_unresolved` alarms and `precons.rev` say exactly what `files` (the
/// complete set, as `(path, body)`) say against the catalog as it is now, in one
/// transaction.
pub fn reconcile_files(conn: &Connection, files: &[(&str, &str)]) -> Result<Report> {
    // Immediate: this reads and then writes, and a deferred transaction that
    // loses that upgrade is refused without waiting out the busy timeout.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let printings = normal_printings(&tx)?;
    tx.execute_batch(
        "DELETE FROM precon_extras; DELETE FROM precon_cards; DELETE FROM precon_decks;",
    )?;

    let mut report = Report::default();
    // Keyed as the alarm table is, so two defects on one line share its row.
    let mut defects: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, body) in files {
        let mut defect = |at: String, what: String| {
            defects
                .entry(format!("{path}:{at}"))
                .or_default()
                .push(what);
        };
        let file: DeckFile = match serde_json::from_str(body) {
            Ok(f) => f,
            Err(e) => {
                defect("0".into(), format!("does not parse: {e}"));
                continue;
            }
        };
        let precon_id = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        let mut offered = true;

        let known_product = tx
            .prepare_cached("SELECT 1 FROM sealed_products WHERE product_id = ?1")?
            .exists([file.product_id])?;
        if !known_product {
            offered = false;
            defect(
                "0".into(),
                format!("product_id {} has no sealed_products row", file.product_id),
            );
        }

        let deck = resolve(&file.cards, &printings, |n, what| {
            offered = false;
            defect(n.to_string(), what);
        });
        let extras = resolve(&file.extras, &printings, |n, what| {
            offered = false;
            defect(format!("extras:{n}"), what);
        });
        let count = deck.count;
        if count != file.total {
            offered = false;
            defect(
                "0".into(),
                format!("total is {}, the lines add up to {count}", file.total),
            );
        }

        tx.execute(
            "INSERT INTO precon_decks (precon_id, product_id, name, deck, set_code, kind, \
                                       source, card_count, offered) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                precon_id,
                file.product_id,
                file.name,
                file.deck,
                file.set_code,
                file.kind.as_str(),
                file.source,
                count,
                offered
            ],
        )?;
        for (n, line, hit) in deck.hits {
            tx.execute(
                "INSERT INTO precon_cards (precon_id, line, printing_id, card_id, zone, qty) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    precon_id,
                    n as i64,
                    hit.printing_id,
                    hit.card_id,
                    line.zone.as_str(),
                    line.qty
                ],
            )?;
        }
        for (n, line, hit) in extras.hits {
            tx.execute(
                "INSERT INTO precon_extras (precon_id, line, printing_id, card_id, qty) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![precon_id, n as i64, hit.printing_id, hit.card_id, line.qty],
            )?;
        }
        report.decks += 1;
        report.offered += usize::from(offered);
    }

    report.alarms = defects
        .into_iter()
        .map(|(key, what)| AlarmRecord::alarm(ALARM_KIND, key, what.join("; ")))
        .collect();
    dumpstar_swu::apply::put_alarms(&tx, &report.alarms, &[ALARM_KIND])?;
    tx.execute(
        "INSERT INTO sync_state (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![REV_KEY, precons_rev(files)],
    )?;
    tx.commit()?;
    Ok(report)
}

/// A preconstructed deck the catalog offers.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Precon {
    pub precon_id: String,
    #[ts(type = "number")]
    pub product_id: i64,
    pub name: String,
    /// The member's name when one product holds two decks.
    pub deck: Option<String>,
    pub set_code: String,
    pub set_name: String,
    /// `spotlight` | `starter` | `twin-suns` | `intro`.
    pub kind: String,
    #[ts(type = "number")]
    pub card_count: i64,
    /// Cards in the product outside the deck; 0 for most precons.
    #[ts(type = "number")]
    pub extra_count: i64,
    /// The art of the deck's first leader line.
    pub leader_image: Option<String>,
}

/// Every offered deck, newest set first, then by name.
pub fn list(conn: &Connection) -> Result<Vec<Precon>> {
    let mut stmt = conn.prepare(
        "SELECT d.precon_id, d.product_id, d.name, d.deck, COALESCE(d.set_code, ''), \
                COALESCE(s.name, d.set_code, ''), d.kind, d.card_count, \
                (SELECT p.image_url FROM shared.precon_cards l \
                   JOIN shared.cards c ON c.card_id = l.card_id \
                   JOIN shared.printings p ON p.printing_id = l.printing_id \
                  WHERE l.precon_id = d.precon_id AND c.card_type = 'Leader' \
                  ORDER BY l.line LIMIT 1), \
                COALESCE((SELECT SUM(qty) FROM shared.precon_extras e \
                           WHERE e.precon_id = d.precon_id), 0) \
           FROM shared.precon_decks d \
           LEFT JOIN shared.sets s ON s.set_code = d.set_code \
          WHERE d.offered \
          ORDER BY s.release_date DESC, d.name, d.deck, d.precon_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Precon {
            precon_id: r.get(0)?,
            product_id: r.get(1)?,
            name: r.get(2)?,
            deck: r.get(3)?,
            set_code: r.get(4)?,
            set_name: r.get(5)?,
            kind: r.get(6)?,
            card_count: r.get(7)?,
            leader_image: r.get(8)?,
            extra_count: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What one [`add`] created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub deck_id: i64,
    pub batch_id: i64,
    /// The copies the deck holds.
    pub copies: u32,
    /// The copies of the rest of the box, held by no deck.
    pub extras: u32,
}

#[cfg(test)]
thread_local! {
    /// Fails [`add`] once all its copies exist, to prove they are rolled back.
    static FAIL_AFTER_COPIES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Add an offered preconstructed deck to the tenant, all or nothing: a batch,
/// one new copy per card in the box, and a deck whose lines reserve exactly the
/// copies of its own cards. The extras are left unreserved.
/// A deck already added is refused unless `confirm_repeat`, which makes a
/// second deck with copies of its own.
pub fn add(conn: &Connection, precon_id: &str, confirm_repeat: bool) -> Result<Added> {
    decks::immediate(conn, |conn| add_in(conn, precon_id, confirm_repeat))
}

fn add_in(conn: &Connection, precon_id: &str, confirm_repeat: bool) -> Result<Added> {
    let (mut name, member, kind): (String, Option<String>, String) = conn
        .query_row(
            "SELECT name, deck, kind FROM shared.precon_decks WHERE precon_id = ?1 AND offered",
            [precon_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound(format!("preconstructed deck '{precon_id}'")))?;
    if let Some(member) = member {
        name = format!("{name} ({member})");
    }
    if !confirm_repeat {
        let earlier: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, name FROM decks WHERE precon_id = ?1 ORDER BY id LIMIT 1",
                [precon_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, earlier)) = earlier {
            return Err(DbError::Conflict(format!(
                "already added as deck '{earlier}' (id {id}); \
                 add again to get a second deck and second copies"
            )));
        }
    }

    let batch_id = batches::create(
        conn,
        &NewBatch {
            batch_type: "precon".into(),
            name: Some(name.clone()),
            ..Default::default()
        },
    )?;
    let format = if kind == Kind::TwinSuns.as_str() {
        "Twin Suns"
    } else {
        "Premier"
    };
    let deck_id = decks::create(
        conn,
        &NewDeck {
            name,
            format: Some(format.into()),
            ..Default::default()
        },
    )?;
    conn.execute(
        "UPDATE decks SET precon_id = ?2 WHERE id = ?1",
        params![deck_id, precon_id],
    )?;

    let lines: Vec<(String, decks::Line)> = conn
        .prepare(
            "SELECT printing_id, card_id, zone, qty FROM shared.precon_cards \
              WHERE precon_id = ?1 ORDER BY line",
        )?
        .query_map([precon_id], |r| {
            let printing_id: String = r.get(0)?;
            let zone: String = r.get(2)?;
            Ok((
                printing_id.clone(),
                decks::Line {
                    card_id: r.get(1)?,
                    zone: decks::zone_from_str(&zone),
                    qty: r.get(3)?,
                    pick: Pick {
                        printing_id: Some(printing_id),
                        ..Default::default()
                    },
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    // Each copy is born in the deck, which is what makes `set_lines_in` reserve
    // it ahead of any copy the tenant already owned.
    let mut copies = 0;
    for (printing_id, line) in &lines {
        let new = NewCopy {
            printing_id: printing_id.clone(),
            source: "precon".into(),
            batch_id: Some(batch_id),
            deck_id: Some(deck_id),
            ..Default::default()
        };
        for _ in 0..line.qty {
            collection::add_in(conn, &new)?;
            copies += 1;
        }
    }

    let lines: Vec<decks::Line> = lines.into_iter().map(|(_, line)| line).collect();
    match decks::set_lines_in(conn, deck_id, &lines)? {
        SetLines::Set { reserved } if reserved == copies => {}
        other => {
            return Err(DbError::Env(format!(
                "preconstructed deck '{precon_id}' added {copies} copies but did not reserve \
                 them: {other:?}"
            )));
        }
    }

    // After the deck's lines are set, so no extra can be reserved by them.
    let extras = add_extras(conn, precon_id, batch_id)?;

    #[cfg(test)]
    if FAIL_AFTER_COPIES.get() {
        return Err(DbError::Env("injected failure after the copies".into()));
    }

    Ok(Added {
        deck_id,
        batch_id,
        copies,
        extras,
    })
}

/// A copy of each card the box holds outside the deck, in the batch and in no
/// deck. Returns how many.
fn add_extras(conn: &Connection, precon_id: &str, batch_id: i64) -> Result<u32> {
    let lines: Vec<(String, u32)> = conn
        .prepare(
            "SELECT printing_id, qty FROM shared.precon_extras \
              WHERE precon_id = ?1 ORDER BY line",
        )?
        .query_map([precon_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut copies = 0;
    for (printing_id, qty) in lines {
        let new = NewCopy {
            printing_id,
            source: "precon".into(),
            batch_id: Some(batch_id),
            ..Default::default()
        };
        for _ in 0..qty {
            collection::add_in(conn, &new)?;
            copies += 1;
        }
    }
    Ok(copies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect_user, open_shared, testkit};

    const PRODUCT: i64 = 9001;

    /// A catalog holding printings JTL 9 and SOR 189 and one deck product.
    fn catalog() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let c = open_shared(&dir.path().join("shared.sqlite")).unwrap();
        testkit::simple(&c, "JTL", "9", "Boba Fett");
        testkit::simple(&c, "SOR", "189", "Test Unit");
        c.execute(
            "INSERT INTO sealed_products (product_id, name, category, fetched_at) \
             VALUES (?1, 'Spotlight Deck', 'sealed', '')",
            [PRODUCT],
        )
        .unwrap();
        (dir, c)
    }

    fn deck(product_id: i64, total: i64, cards: &[(&str, &str, i64)]) -> String {
        let cards: Vec<_> = cards
            .iter()
            .map(|(set, number, qty)| serde_json::json!({"set": set, "number": number, "qty": qty}))
            .collect();
        serde_json::json!({
            "product_id": product_id, "name": "Test Deck", "set_code": "JTL",
            "kind": "spotlight", "source": "https://example.invalid/deck",
            "total": total, "cards": cards,
        })
        .to_string()
    }

    /// `body` with `extras`: the cards in the box that are not in the deck.
    fn boxed(body: &str, extras: &[(&str, &str, i64)]) -> String {
        let mut file: serde_json::Value = serde_json::from_str(body).unwrap();
        file["extras"] = extras
            .iter()
            .map(|(set, number, qty)| serde_json::json!({"set": set, "number": number, "qty": qty}))
            .collect();
        file.to_string()
    }

    fn good() -> String {
        deck(PRODUCT, 4, &[("JTL", "009", 1), ("SOR", "189", 3)])
    }

    fn offered(c: &Connection, id: &str) -> bool {
        c.query_row(
            "SELECT offered FROM precon_decks WHERE precon_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn alarm_keys(c: &Connection) -> Vec<String> {
        dumpstar_swu::check::stored(c)
            .unwrap()
            .into_iter()
            .filter(|a| a.kind == "precon_unresolved")
            .map(|a| a.key)
            .collect()
    }

    fn rev(c: &Connection) -> String {
        c.query_row(
            "SELECT value FROM sync_state WHERE key = 'precons.rev'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn resolves_every_line_to_a_standard_normal_printing() {
        let (_d, c) = catalog();
        testkit::printing(
            &c,
            "JTL-9-hyper",
            "JTL-9",
            "JTL",
            "9",
            "Hyperspace",
            "Normal",
        );
        testkit::foil_of(&c, "JTL", "9");
        let body = good();
        reconcile_files(&c, &[("data/precons/a.json", &body)]).unwrap();
        let ids: Vec<String> = c
            .prepare("SELECT printing_id FROM precon_cards ORDER BY line")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids, ["JTL-9-normal", "SOR-189-normal"]);
        let count: i64 = c
            .query_row(
                "SELECT card_count FROM precon_decks WHERE precon_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
        assert!(offered(&c, "a"));
        assert!(alarm_keys(&c).is_empty());
    }

    #[test]
    fn unknown_printing_raises_precon_unresolved_naming_file_and_line() {
        let (_d, c) = catalog();
        let bad = deck(PRODUCT, 4, &[("JTL", "009", 1), ("SOR", "999", 3)]);
        let ok = good();
        reconcile_files(
            &c,
            &[
                ("data/precons/bad.json", &bad),
                ("data/precons/ok.json", &ok),
            ],
        )
        .unwrap();
        assert_eq!(alarm_keys(&c), ["data/precons/bad.json:2"]);
        assert!(!offered(&c, "bad"));
        assert!(offered(&c, "ok"));
        let resolved: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM precon_cards WHERE precon_id='bad'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved, 1);
    }

    #[test]
    fn unknown_product_and_wrong_total_raise_precon_unresolved() {
        let (_d, c) = catalog();
        let product = deck(1, 4, &[("JTL", "9", 1), ("SOR", "189", 3)]);
        let total = deck(PRODUCT, 52, &[("JTL", "9", 1), ("SOR", "189", 3)]);
        let ok = good();
        reconcile_files(
            &c,
            &[
                ("data/precons/ok.json", &ok),
                ("data/precons/product.json", &product),
                ("data/precons/total.json", &total),
            ],
        )
        .unwrap();
        assert_eq!(
            alarm_keys(&c),
            ["data/precons/product.json:0", "data/precons/total.json:0"]
        );
        assert!(!offered(&c, "product"));
        assert!(!offered(&c, "total"));
        assert!(offered(&c, "ok"));
    }

    #[test]
    fn an_unparseable_file_raises_its_alarm_and_the_others_still_load() {
        let (_d, c) = catalog();
        let ok = good();
        reconcile_files(
            &c,
            &[
                ("data/precons/broken.json", "{"),
                ("data/precons/ok.json", &ok),
            ],
        )
        .unwrap();
        assert_eq!(alarm_keys(&c), ["data/precons/broken.json:0"]);
        let ids: Vec<String> = c
            .prepare("SELECT precon_id FROM precon_decks")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids, ["ok"]);
    }

    #[test]
    fn fixed_file_retires_its_alarm() {
        let (_d, c) = catalog();
        let bad = deck(PRODUCT, 4, &[("JTL", "9", 1), ("SOR", "999", 3)]);
        reconcile_files(&c, &[("data/precons/a.json", &bad)]).unwrap();
        assert_eq!(alarm_keys(&c), ["data/precons/a.json:2"]);
        let fixed = good();
        reconcile_files(&c, &[("data/precons/a.json", &fixed)]).unwrap();
        assert!(alarm_keys(&c).is_empty());
        assert!(offered(&c, "a"));
    }

    #[test]
    fn changing_a_decklist_changes_precons_rev_and_rebuilds() {
        let (_d, c) = catalog();
        let a = good();
        reconcile_files(&c, &[("data/precons/a.json", &a)]).unwrap();
        let before = rev(&c);
        let changed = deck(PRODUCT, 3, &[("JTL", "009", 1), ("SOR", "189", 2)]);
        reconcile_files(&c, &[("data/precons/a.json", &changed)]).unwrap();
        assert_ne!(rev(&c), before);
        let qty: i64 = c
            .query_row("SELECT qty FROM precon_cards WHERE line = 2", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(qty, 2);
    }

    #[test]
    fn opening_the_catalog_reconciles_the_shipped_files() {
        let (_d, c) = catalog();
        assert_eq!(rev(&c), precons_rev(PRECON_FILES));
        let decks: usize = c
            .query_row("SELECT COUNT(*) FROM precon_decks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(decks, PRECON_FILES.len());
    }

    #[test]
    fn a_line_without_a_positive_qty_is_a_defect() {
        let (_d, c) = catalog();
        let bad = deck(PRODUCT, 1, &[("JTL", "9", 1), ("SOR", "189", 0)]);
        let ok = good();
        reconcile_files(
            &c,
            &[
                ("data/precons/bad.json", &bad),
                ("data/precons/ok.json", &ok),
            ],
        )
        .unwrap();
        assert_eq!(alarm_keys(&c), ["data/precons/bad.json:2"]);
        assert!(!offered(&c, "bad"));
        assert!(offered(&c, "ok"));
        let lines: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM precon_cards WHERE precon_id='bad'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lines, 1);
    }

    #[test]
    fn extras_resolve_and_an_unresolved_extra_alarms() {
        let (_d, c) = catalog();
        let body = boxed(&good(), &[("SOR", "189", 2)]);
        reconcile_files(&c, &[("data/precons/a.json", &body)]).unwrap();
        assert!(offered(&c, "a"));
        let rows: (i64, i64, i64, i64) = c
            .query_row(
                "SELECT (SELECT COUNT(*) FROM precon_cards), \
                        (SELECT COUNT(*) FROM precon_extras), \
                        (SELECT SUM(qty) FROM precon_extras), \
                        (SELECT card_count FROM precon_decks)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(rows, (2, 1, 2, 4));
        assert!(alarm_keys(&c).is_empty());

        let bad = boxed(&good(), &[("SOR", "189", 2), ("SOR", "999", 1)]);
        reconcile_files(&c, &[("data/precons/a.json", &bad)]).unwrap();
        assert!(!offered(&c, "a"));
        assert_eq!(alarm_keys(&c), ["data/precons/a.json:extras:2"]);
    }

    #[test]
    fn an_extra_without_a_positive_qty_is_a_defect() {
        let (_d, c) = catalog();
        let bad = boxed(&good(), &[("SOR", "189", 0)]);
        reconcile_files(&c, &[("data/precons/a.json", &bad)]).unwrap();
        assert!(!offered(&c, "a"));
        assert_eq!(alarm_keys(&c), ["data/precons/a.json:extras:1"]);
    }

    #[test]
    fn a_number_above_the_set_size_derives_and_adds_its_hyperspace_printing() {
        let (dir, c) = catalog();
        testkit::set(&c, "IC27", "Icons 2027");
        testkit::printing(
            &c,
            "IC27-209-hyper",
            "SOR-189",
            "IC27",
            "209",
            "Hyperspace",
            "Normal",
        );
        let body = boxed(
            &deck(PRODUCT, 1, &[("IC27", "209", 1)]),
            &[("IC27", "209", 1)],
        );
        reconcile_files(&c, &[("data/precons/ic.json", &body)]).unwrap();
        assert!(alarm_keys(&c).is_empty(), "{:?}", alarm_keys(&c));
        drop(c);
        let c = connect(&dir);

        let added = add(&c, "ic", false).unwrap();

        let printings: Vec<String> = c
            .prepare("SELECT printing_id FROM collection WHERE batch_id = ?1")
            .unwrap()
            .query_map([added.batch_id], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(printings, ["IC27-209-hyper", "IC27-209-hyper"]);
        assert_eq!(held(&c, added.deck_id).len(), 1);
    }

    /// Promo sets put different cards on one number, none of them Standard.
    #[test]
    fn a_number_shared_by_variants_with_no_standard_one_alarms() {
        let (_d, c) = catalog();
        testkit::set(&c, "SORP", "Promos");
        for (id, variant) in [
            ("SORP-1-weekly", "Weekly Play"),
            ("SORP-1-judge", "SS Judge"),
        ] {
            testkit::printing(&c, id, "SOR-189", "SORP", "1", variant, "Normal");
        }
        let body = deck(PRODUCT, 1, &[("SORP", "1", 1)]);
        reconcile_files(&c, &[("data/precons/a.json", &body)]).unwrap();
        assert!(!offered(&c, "a"));
        assert_eq!(alarm_keys(&c), ["data/precons/a.json:1"]);
    }

    /// The derive's case: the product exists only as a tcgcsv row until the
    /// same convergence folds it into `sealed_products`.
    #[test]
    fn convergence_reconciles_after_folding_the_sealed_products() {
        let (_d, mut c) = catalog();
        c.execute_batch(
            "INSERT INTO tcgcsv_groups (group_id, name, abbreviation) VALUES (1, 'JTL', 'JTL'); \
             INSERT INTO tcgcsv_products (product_id, group_id, name, number) \
             VALUES (614673, 1, 'Spotlight Deck: Boba Fett', NULL);",
        )
        .unwrap();
        crate::converge_shared(&mut c).unwrap();
        let keys = alarm_keys(&c);
        assert!(!keys.contains(&"data/precons/jtl-boba-fett.json:0".to_string()));
        // The file is being judged at all: its cards are not in this catalog.
        assert!(keys.contains(&"data/precons/jtl-boba-fett.json:2".to_string()));
    }

    #[test]
    fn the_shipped_files_reconcile() {
        let (_, body) = PRECON_FILES
            .iter()
            .find(|(p, _)| *p == "data/precons/jtl-boba-fett.json")
            .expect("the build script embeds data/precons/jtl-boba-fett.json");
        let boba: DeckFile = serde_json::from_str(body).unwrap();
        assert_eq!(boba.kind.as_str(), "spotlight");
        assert_eq!(boba.product_id, 614673);
    }

    /// A tenant over [`catalog`] (JTL 9 and SOR 189) holding the test deck
    /// `test`: one copy of JTL 9 and three of SOR 189.
    fn tenant() -> (tempfile::TempDir, Connection) {
        tenant_of(&good())
    }

    /// A tenant over [`catalog`] holding `body` as the deck `test`.
    fn tenant_of(body: &str) -> (tempfile::TempDir, Connection) {
        let (dir, c) = catalog();
        reconcile_files(&c, &[("data/precons/test.json", body)]).unwrap();
        drop(c);
        let conn = connect(&dir);
        (dir, conn)
    }

    fn connect(dir: &tempfile::TempDir) -> Connection {
        connect_user(
            &dir.path().join("collection.sqlite"),
            &dir.path().join("shared.sqlite"),
        )
        .unwrap()
    }

    /// `(batches, copies, decks)` the tenant holds.
    fn counts(c: &Connection) -> (i64, i64, i64) {
        c.query_row(
            "SELECT (SELECT COUNT(*) FROM batches), (SELECT COUNT(*) FROM collection), \
                    (SELECT COUNT(*) FROM decks)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    }

    fn ids(c: &Connection, sql: &str, arg: i64) -> Vec<i64> {
        c.prepare(sql)
            .unwrap()
            .query_map([arg], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn held(c: &Connection, deck: i64) -> Vec<i64> {
        ids(
            c,
            "SELECT id FROM collection WHERE deck_id = ?1 ORDER BY id",
            deck,
        )
    }

    #[test]
    fn add_makes_one_batch_n_copies_one_deck_and_reserves_exactly_them() {
        let (_d, mut c) = tenant();
        let owned = collection::add(
            &mut c,
            &NewCopy {
                printing_id: "SOR-189-normal".into(),
                source: "manual_id".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let added = add(&c, "test", false).unwrap();

        assert_eq!(counts(&c), (1, 5, 1));
        assert_eq!(added.copies, 4);
        let new = ids(
            &c,
            "SELECT id FROM collection WHERE batch_id = ?1 ORDER BY id",
            added.batch_id,
        );
        assert_eq!(new.len(), 4);
        assert!(!new.contains(&owned));
        assert_eq!(held(&c, added.deck_id), new);
        let free: Option<i64> = c
            .query_row(
                "SELECT deck_id FROM collection WHERE id = ?1",
                [owned],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(free, None);
        let deck = decks::get(&c, added.deck_id).unwrap().unwrap();
        assert_eq!(deck.precon_id.as_deref(), Some("test"));
        assert_eq!(deck.name, "Test Deck");
        assert_eq!(deck.format.as_deref(), Some("Premier"));
        let batch = crate::batches::get_detail(&c, added.batch_id)
            .unwrap()
            .unwrap()
            .batch;
        assert_eq!(batch.batch_type, "precon");
        assert_eq!(batch.name.as_deref(), Some("Test Deck"));
        let lines = decks::line_states(&c, added.deck_id).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.reserved == l.qty), "{lines:?}");
    }

    #[test]
    fn adding_a_box_with_extras_reserves_only_the_deck() {
        let (_d, c) = tenant_of(&boxed(&good(), &[("SOR", "189", 2)]));

        let added = add(&c, "test", false).unwrap();

        assert_eq!((added.copies, added.extras), (4, 2));
        assert_eq!(counts(&c), (1, 6, 1));
        let batch = ids(
            &c,
            "SELECT id FROM collection WHERE batch_id = ?1 ORDER BY id",
            added.batch_id,
        );
        assert_eq!(batch.len(), 6);
        let lines = decks::line_states(&c, added.deck_id).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.reserved == l.qty), "{lines:?}");
        assert_eq!(lines.iter().map(|l| l.reserved).sum::<u32>(), 4);
        let deck_copies = held(&c, added.deck_id);
        assert_eq!(deck_copies.len(), 4);
        assert!(deck_copies.iter().all(|id| batch.contains(id)));
        let free = ids(
            &c,
            "SELECT id FROM collection WHERE batch_id = ?1 AND deck_id IS NULL",
            added.batch_id,
        );
        assert_eq!(free.len(), 2);
        let listed = list(&c).unwrap();
        assert_eq!((listed[0].card_count, listed[0].extra_count), (4, 2));
    }

    #[test]
    fn a_failure_after_the_extras_leaves_nothing() {
        let (_d, c) = tenant_of(&boxed(&good(), &[("SOR", "189", 2)]));
        let before = counts(&c);
        FAIL_AFTER_COPIES.set(true);
        let out = add(&c, "test", false);
        FAIL_AFTER_COPIES.set(false);
        let err = out.unwrap_err().to_string();
        assert!(err.contains("injected"), "{err}");
        assert_eq!(counts(&c), before);
    }

    #[test]
    fn a_failure_after_the_copies_leaves_nothing() {
        let (_d, c) = tenant();
        let before = counts(&c);
        FAIL_AFTER_COPIES.set(true);
        let out = add(&c, "test", false);
        FAIL_AFTER_COPIES.set(false);
        let err = out.unwrap_err().to_string();
        assert!(err.contains("injected"), "{err}");
        assert_eq!(counts(&c), before);
    }

    #[test]
    fn a_repeat_add_is_refused_naming_the_earlier_deck_and_confirm_makes_a_second() {
        let (_d, c) = tenant();
        let first = add(&c, "test", false).unwrap();
        let before = counts(&c);

        let err = add(&c, "test", false).unwrap_err();
        let DbError::Conflict(msg) = err else {
            panic!("not a conflict: {err}");
        };
        assert!(
            msg.contains("'Test Deck'") && msg.contains(&format!("(id {})", first.deck_id)),
            "{msg}"
        );
        assert_eq!(counts(&c), before);

        let first_copies = held(&c, first.deck_id);
        let second = add(&c, "test", true).unwrap();
        assert_ne!(second.deck_id, first.deck_id);
        assert_eq!(counts(&c), (2, 8, 2));
        assert_eq!(held(&c, first.deck_id), first_copies);
        assert_eq!(held(&c, second.deck_id).len(), 4);
    }

    #[test]
    fn a_twin_suns_deck_with_two_leaders_is_legal_after_the_add() {
        let (dir, c) = catalog();
        dumpstar_swu::Catalog::open_writer(&dir.path().join("shared.sqlite"))
            .unwrap()
            .seed_formats()
            .unwrap();
        testkit::set(&c, "TS26", "Twin Suns");
        let mut cards = Vec::new();
        for n in 1..=83 {
            let number = n.to_string();
            let card_id = format!("TS26-{n}");
            let kind = match n {
                1 | 2 => "Leader",
                3 => "Base",
                _ => "Unit",
            };
            c.execute(
                "INSERT INTO cards (card_id, name, card_type) VALUES (?1, ?1, ?2)",
                [card_id.as_str(), kind],
            )
            .unwrap();
            testkit::printing(
                &c,
                &format!("{card_id}-normal"),
                &card_id,
                "TS26",
                &number,
                "Standard",
                "Normal",
            );
            cards.push(("TS26", number));
        }
        let cards: Vec<_> = cards.iter().map(|(s, n)| (*s, n.as_str(), 1)).collect();
        let body = deck(PRODUCT, 83, &cards).replace("\"spotlight\"", "\"twin-suns\"");
        reconcile_files(&c, &[("data/precons/suns.json", &body)]).unwrap();
        drop(c);
        let c = connect(&dir);

        let added = add(&c, "suns", false).unwrap();

        let v = decks::validate(&c, added.deck_id).unwrap();
        assert_eq!(v.format_id.as_deref(), Some("twin_suns"));
        let report = v.report.unwrap();
        assert!(report.legal, "{:?}", report.violations);
        assert_eq!(report.counts.leaders, 2);
    }

    #[test]
    fn a_deck_not_offered_cannot_be_added() {
        let (dir, c) = catalog();
        let bad = deck(PRODUCT, 52, &[("JTL", "009", 1), ("SOR", "189", 3)]);
        reconcile_files(&c, &[("data/precons/bad.json", &bad)]).unwrap();
        assert!(!offered(&c, "bad"));
        drop(c);
        let c = connect(&dir);
        for id in ["bad", "no-such-deck"] {
            let err = add(&c, id, false).unwrap_err();
            assert!(matches!(err, DbError::NotFound(_)), "{id}: {err}");
        }
        assert_eq!(counts(&c), (0, 0, 0));
        assert!(list(&c).unwrap().is_empty());
    }

    #[test]
    fn list_shows_offered_decks_newest_set_first_with_leader_art() {
        let (dir, c) = catalog();
        testkit::set_on(&c, "OLD", "Older Set", "2023-01-01");
        c.execute_batch(
            "UPDATE cards SET card_type = 'Leader' WHERE card_id = 'JTL-9'; \
             UPDATE printings SET image_url = 'https://example.invalid/boba.png' \
              WHERE printing_id = 'JTL-9-normal';",
        )
        .unwrap();
        let new = good();
        let named = deck(PRODUCT, 3, &[("SOR", "189", 3)])
            .replace("\"set_code\":\"JTL\"", "\"set_code\":\"OLD\"")
            .replace(
                "\"name\":\"Test Deck\"",
                "\"name\":\"A Deck\",\"deck\":\"Vader\"",
            );
        let bad = deck(PRODUCT, 9, &[("SOR", "189", 3)]);
        // Same set as `new`, sorting before it by name and after it by id.
        let first = good().replace("\"Test Deck\"", "\"Alpha Deck\"");
        reconcile_files(
            &c,
            &[
                ("data/precons/zz.json", &first),
                ("data/precons/bad.json", &bad),
                ("data/precons/named.json", &named),
                ("data/precons/new.json", &new),
            ],
        )
        .unwrap();
        drop(c);
        let c = connect(&dir);

        let got = list(&c).unwrap();

        let ids: Vec<_> = got.iter().map(|p| p.precon_id.as_str()).collect();
        assert_eq!(ids, ["zz", "new", "named"]);
        assert_eq!(got[1].set_name, "JTL");
        assert_eq!(got[1].kind, "spotlight");
        assert_eq!(got[1].card_count, 4);
        assert!(got.iter().all(|p| p.extra_count == 0));
        assert_eq!(
            got[1].leader_image.as_deref(),
            Some("https://example.invalid/boba.png")
        );
        assert_eq!(got[2].set_name, "Older Set");
        assert_eq!(got[2].deck.as_deref(), Some("Vader"));
        assert_eq!(got[2].leader_image, None);

        let added = add(&c, "named", false).unwrap();
        let deck = decks::get(&c, added.deck_id).unwrap().unwrap();
        assert_eq!(deck.name, "A Deck (Vader)");
    }
}
