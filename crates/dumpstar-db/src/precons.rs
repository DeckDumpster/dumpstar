//! Preconstructed decklists, a catalog source authored in git (`data/precons/`).
//!
//! The files are compiled in by `build.rs`, so a new deck is a new file and no
//! code edit. Every convergence reconciles them into `precon_decks` and
//! `precon_cards`: each line names the Standard Normal printing of a set and
//! collector number, and a line can start resolving when a derive brings the
//! printing in, which is why this runs every time rather than once per revision.
//!
//! A defect never fails the build. It raises a `precon_unresolved` alarm keyed
//! `<file>:<line>` (1-based card line; `0` for the file as a whole), which
//! `catalog-check` reports, and the deck is stored `offered = 0`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use dumpstar_swu::crosswalk::norm_number;
use dumpstar_swu::record::AlarmRecord;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::convergence::hash_inputs;
use crate::error::Result;

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

/// `(set_code, normalised number)` to `(printing_id, card_id)`.
type Printings = HashMap<(String, String), (String, String)>;

fn standard_normal_printings(tx: &Transaction) -> Result<Printings> {
    let mut stmt = tx.prepare(
        "SELECT set_code, number, printing_id, card_id FROM printings \
          WHERE variant = 'Standard' AND finish = 'Normal' \
          ORDER BY variant_code, printing_id",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Printings::new();
    while let Some(r) = rows.next()? {
        let number: String = r.get(1)?;
        out.entry((r.get(0)?, norm_number(&number)))
            .or_insert((r.get(2)?, r.get(3)?));
    }
    Ok(out)
}

/// Make `precon_decks`, `precon_cards`, the `precon_unresolved` alarms and
/// `precons.rev` say exactly what `files` (the complete set, as
/// `(path, body)`) say against the catalog as it is now, in one transaction.
pub fn reconcile_files(conn: &Connection, files: &[(&str, &str)]) -> Result<Report> {
    // Immediate: this reads and then writes, and a deferred transaction that
    // loses that upgrade is refused without waiting out the busy timeout.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let printings = standard_normal_printings(&tx)?;
    tx.execute_batch("DELETE FROM precon_cards; DELETE FROM precon_decks;")?;

    let mut report = Report::default();
    // Keyed as the alarm table is, so two defects on one line share its row.
    let mut defects: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, body) in files {
        let mut defect = |line: usize, what: String| {
            defects
                .entry(format!("{path}:{line}"))
                .or_default()
                .push(what);
        };
        let file: DeckFile = match serde_json::from_str(body) {
            Ok(f) => f,
            Err(e) => {
                defect(0, format!("does not parse: {e}"));
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
                0,
                format!("product_id {} has no sealed_products row", file.product_id),
            );
        }

        let mut count: i64 = 0;
        let mut resolved = Vec::new();
        for (i, line) in file.cards.iter().enumerate() {
            let n = i + 1;
            if line.qty < 1 {
                offered = false;
                defect(n, format!("qty {} is not a positive count", line.qty));
                continue;
            }
            count = count.saturating_add(line.qty);
            match printings.get(&(line.set.clone(), norm_number(&line.number))) {
                Some(hit) => resolved.push((n, line, hit)),
                None => {
                    offered = false;
                    defect(
                        n,
                        format!("no Standard Normal printing {} {}", line.set, line.number),
                    );
                }
            }
        }
        if count != file.total {
            offered = false;
            defect(
                0,
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
        for (n, line, (printing_id, card_id)) in resolved {
            tx.execute(
                "INSERT INTO precon_cards (precon_id, line, printing_id, card_id, zone, qty) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    precon_id,
                    n as i64,
                    printing_id,
                    card_id,
                    line.zone.as_str(),
                    line.qty
                ],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open_shared, testkit};

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
}
