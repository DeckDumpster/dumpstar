//! Read access to the set catalog, with per-set collection progress.

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::search_meta::RarityLookup;

/// A set with its card count and how many of its cards the user owns —
/// the shape the `/browse` set picker renders.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SetSummary {
    pub set_code: String,
    pub name: String,
    pub release_date: Option<String>,
    /// SWU product category ("Main Expansions", "Weekly Play Promos"…); the
    /// list arrives grouped by it, main expansions first. `null` only for a
    /// set the catalog's declared mapping lacks (an alarm is raised for it).
    pub category: Option<String>,
    /// Distinct cards printed in the set.
    #[ts(type = "number")]
    pub total_cards: i64,
    /// Distinct cards in the set the user owns at least one copy of.
    #[ts(type = "number")]
    pub owned_cards: i64,
    /// Printings in the set (the master set), and how many are owned.
    #[ts(type = "number")]
    pub total_printings: i64,
    #[ts(type = "number")]
    pub owned_printings: i64,
}

/// List every set grouped by category (main expansions first), each
/// category newest first, with card and owned-card counts. Requires a
/// user connection (the owned count joins the collection).
pub fn list_sets(conn: &Connection) -> Result<Vec<SetSummary>> {
    let mut stmt = conn.prepare(
        "SELECT s.set_code, s.name, s.release_date, s.category, \
                (SELECT count(DISTINCT p.card_id) FROM printings p WHERE p.set_code = s.set_code), \
                (SELECT count(DISTINCT p.card_id) FROM collection c \
                   JOIN printings p ON c.printing_id = p.printing_id \
                  WHERE p.set_code = s.set_code), \
                (SELECT count(*) FROM printings p WHERE p.set_code = s.set_code), \
                (SELECT count(DISTINCT c.printing_id) FROM collection c \
                   JOIN printings p ON c.printing_id = p.printing_id \
                  WHERE p.set_code = s.set_code) \
         FROM sets s \
         ORDER BY s.release_date DESC NULLS LAST, s.sort_order DESC, s.set_code",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SetSummary {
            set_code: r.get(0)?,
            name: r.get(1)?,
            release_date: r.get(2)?,
            category: r.get(3)?,
            total_cards: r.get(4)?,
            owned_cards: r.get(5)?,
            total_printings: r.get(6)?,
            owned_printings: r.get(7)?,
        })
    })?;
    let mut sets: Vec<SetSummary> = rows.collect::<rusqlite::Result<_>>()?;
    // Stable: within a category the newest-first order of the query holds.
    sets.sort_by_key(|s| dumpstar_catalog::set_categories::category_rank(s.category.as_deref()));
    Ok(sets)
}

/// One rarity tier within a set, with how many of its cards the user owns.
///
/// `rarity` is the canonical spelling from the `rarities` table, and
/// `rank`/`grp` come from the same row: the tier's curated ordinal and its
/// group alias. Both ride along so the stats page can order and colour the
/// split without re-deriving a rarity typology in TypeScript — the catalog
/// already owns one.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RarityCount {
    pub rarity: String,
    /// Curated ordinal from `rarities.rank`; [`UNRANKED_RARITY`] for a
    /// tier the table does not carry. Rows arrive pre-sorted by it.
    #[ts(type = "number")]
    pub rank: i64,
    /// Group alias (`common`, `ultra`, `secret`…), or `null` when the
    /// tier is unranked or the row declares no group.
    pub grp: Option<String>,
    #[ts(type = "number")]
    pub total_cards: i64,
    #[ts(type = "number")]
    pub owned_cards: i64,
}

/// Per-card copy count for the stats-page histogram. One row per
/// catalogued card in the set, in collector-number order; `copies` is
/// the total physical count across every variant.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CardCopyCount {
    pub number: String,
    #[ts(type = "number")]
    pub sort_key: i64,
    /// Canonical spelling from the `rarities` table where the tier is
    /// known, else the raw catalog string.
    pub rarity: Option<String>,
    /// Group alias for the tier — the histogram paints its columns by it.
    pub rarity_grp: Option<String>,
    #[ts(type = "number")]
    pub copies: i64,
}

/// Analytical breakdown of a single set: completion by card and by printing
/// (the master set), the rarity split, and value — the full set's market
/// value and the value owned.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SetAnalytics {
    pub set_code: String,
    pub name: String,
    pub release_date: Option<String>,
    /// Distinct cards printed in the set, and how many the user owns (any
    /// printing counts).
    #[ts(type = "number")]
    pub total_cards: i64,
    #[ts(type = "number")]
    pub owned_cards: i64,
    /// Printings in the set (the master set), and how many are owned.
    #[ts(type = "number")]
    pub total_printings: i64,
    #[ts(type = "number")]
    pub owned_printings: i64,
    /// Total physical copies of any card in the set — sums duplicates.
    #[ts(type = "number")]
    pub owned_copies: i64,
    /// Market value: the cheapest printing of every card in the set, one of
    /// every printing the user owns, and the all-physical-copies sum (counts
    /// duplicates).
    pub market_value: f64,
    pub owned_value_unique: f64,
    pub owned_value: f64,
    pub rarities: Vec<RarityCount>,
    /// One entry per card in the set, ordered by collector number —
    /// drives the stats-page copy-count histogram.
    pub copy_counts: Vec<CardCopyCount>,
}

/// Fold raw `(rarity, total, owned)` tallies onto their canonical tiers
/// and return them in the catalog's curated order.
///
/// Sorting here rather than in the client is the point: the ordinal lives
/// in `data/rarities.json` and nowhere else, so the stats table renders
/// the rows in the order it receives them.
pub(crate) fn rank_rarities(
    tiers: &RarityLookup,
    raw_counts: Vec<(String, i64, i64)>,
) -> Vec<RarityCount> {
    let mut folded: std::collections::HashMap<String, RarityCount> =
        std::collections::HashMap::new();
    for (raw, total, owned) in raw_counts {
        let name = tiers.display(&raw);
        let entry = folded.entry(name.clone()).or_insert_with(|| RarityCount {
            rarity: name,
            rank: tiers.rank(&raw),
            grp: tiers.grp(&raw),
            total_cards: 0,
            owned_cards: 0,
        });
        entry.total_cards += total;
        entry.owned_cards += owned;
    }
    let mut out: Vec<RarityCount> = folded.into_values().collect();
    out.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.rarity.cmp(&b.rarity)));
    out
}

/// Compute the analytical breakdown for one set. `None` if no such set.
pub fn analytics(conn: &Connection, set_code: &str) -> Result<Option<SetAnalytics>> {
    let header: Option<(String, Option<String>)> = conn
        .prepare("SELECT name, release_date FROM sets WHERE set_code = ?1")?
        .query_row([set_code], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((name, release_date)) = header else {
        return Ok(None);
    };

    let one = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [set_code], |r| r.get(0))?) };
    let total_cards = one("SELECT count(DISTINCT card_id) FROM printings WHERE set_code = ?1")?;
    let owned_cards = one("SELECT count(DISTINCT p.card_id) FROM collection c \
           JOIN printings p ON c.printing_id = p.printing_id WHERE p.set_code = ?1")?;
    let total_printings = one("SELECT count(*) FROM printings WHERE set_code = ?1")?;
    let owned_printings = one("SELECT count(DISTINCT c.printing_id) FROM collection c \
           JOIN printings p ON c.printing_id = p.printing_id WHERE p.set_code = ?1")?;
    // Total physical copies in the set (one row per copy in `collection`).
    let owned_copies = one("SELECT count(*) FROM collection col \
           JOIN printings p ON col.printing_id = p.printing_id WHERE p.set_code = ?1")?;

    // Effective market price for a printing `p` — one rule, defined once in
    // `crate::prices`. Everything this query sees is a *catalog* printing, so
    // a tenant's own manual price cannot reach it (pd-m4gw).
    let price_expr = crate::prices::MARKET_PRICE_EXPR;
    // "Full set" market value = the minimum cost to own one of every card,
    // i.e. the CHEAPEST printing per card summed across cards — NOT the sum
    // of every printing. A card prints as Standard, Hyperspace and Showcase,
    // each in Normal and Foil; summing them all would be dominated by the
    // priciest and say nothing about completing the set.
    let market_value: f64 = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(card_min), 0) FROM ( \
               SELECT MIN({price_expr}) AS card_min \
               FROM printings p WHERE p.set_code = ?1 \
               GROUP BY p.card_id \
             )"
        ),
        [set_code],
        |r| r.get(0),
    )?;
    let owned_value: f64 = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM({price_expr}), 0) \
             FROM collection col JOIN printings p ON col.printing_id = p.printing_id \
             WHERE p.set_code = ?1"
        ),
        [set_code],
        |r| r.get(0),
    )?;
    // One-of-each owned printing value — the "completion picture" sum
    // that doesn't double-count duplicates.
    let owned_value_unique: f64 = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM({price_expr}), 0) \
             FROM printings p \
             WHERE p.set_code = ?1 \
               AND EXISTS (SELECT 1 FROM collection col WHERE col.printing_id = p.printing_id)"
        ),
        [set_code],
        |r| r.get(0),
    )?;

    // The rarity typology — canonical spelling, curated rank, group alias —
    // comes from the shared catalog's `rarities` table, not from whatever
    // string the upstream row happened to carry.
    let tiers = RarityLookup::load(conn)?;

    // Per-card copy counts in collector-number order, one row per card: the
    // card's lead printing (Standard first) names its number and rarity.
    // LEFT JOIN through collection so cards owned 0 times still produce a row.
    let copy_counts: Vec<CardCopyCount> = {
        let mut stmt = conn.prepare(
            "SELECT lead.number, lead.rarity, \
                    (SELECT COUNT(col.id) FROM collection col \
                       JOIN printings q ON q.printing_id = col.printing_id \
                      WHERE q.card_id = lead.card_id AND q.set_code = ?1) AS copies \
             FROM printings lead \
             WHERE lead.set_code = ?1 \
               AND lead.printing_id = (SELECT x.printing_id FROM printings x \
                                        WHERE x.card_id = lead.card_id AND x.set_code = ?1 \
                                        ORDER BY x.variant <> 'Standard', x.finish <> 'Normal', \
                                                 x.printing_id LIMIT 1)",
        )?;
        let rows = stmt.query_map([set_code], |r| {
            let number: String = r.get(0)?;
            let raw: Option<String> = r.get(1)?;
            Ok(CardCopyCount {
                sort_key: dumpstar_core::number_sort_key(&number),
                number,
                rarity: raw.as_deref().map(|s| tiers.display(s)),
                rarity_grp: raw.as_deref().and_then(|s| tiers.grp(s)),
                copies: r.get(2)?,
            })
        })?;
        let mut rows: Vec<CardCopyCount> = rows.collect::<rusqlite::Result<_>>()?;
        rows.sort_by(|a, b| {
            a.sort_key
                .cmp(&b.sort_key)
                .then_with(|| a.number.cmp(&b.number))
        });
        rows
    };

    // Tally the lead printings' raw rarity strings, then fold them onto the
    // canonical spelling in Rust.
    let rarities = {
        let mut stmt = conn.prepare(
            "SELECT COALESCE(lead.rarity, 'Unknown') AS rarity, \
                    count(*) AS total_cards, \
                    sum(EXISTS (SELECT 1 FROM collection col \
                                  JOIN printings q ON q.printing_id = col.printing_id \
                                 WHERE q.card_id = lead.card_id AND q.set_code = ?1)) \
             FROM printings lead \
             WHERE lead.set_code = ?1 \
               AND lead.printing_id = (SELECT x.printing_id FROM printings x \
                                        WHERE x.card_id = lead.card_id AND x.set_code = ?1 \
                                        ORDER BY x.variant <> 'Standard', x.finish <> 'Normal', \
                                                 x.printing_id LIMIT 1) \
             GROUP BY rarity",
        )?;
        let rows = stmt.query_map([set_code], |r| {
            let raw: String = r.get(0)?;
            let total: i64 = r.get(1)?;
            let owned: i64 = r.get(2)?;
            Ok((raw, total, owned))
        })?;
        rank_rarities(&tiers, rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };

    Ok(Some(SetAnalytics {
        set_code: set_code.to_string(),
        name,
        release_date,
        total_cards,
        owned_cards,
        total_printings,
        owned_printings,
        owned_copies,
        market_value,
        owned_value_unique,
        owned_value,
        rarities,
        copy_counts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::{self, NewCopy};
    use crate::testkit;
    use crate::{connect_user, open_shared};

    fn rarity(c: &Connection, printing_id: &str, rarity: &str) {
        c.execute(
            "UPDATE printings SET rarity = ?2 WHERE printing_id = ?1",
            rusqlite::params![printing_id, rarity],
        )
        .unwrap();
    }

    fn own(conn: &mut Connection, pids: &[&str]) {
        for pid in pids {
            collection::add(
                conn,
                &NewCopy {
                    printing_id: (*pid).into(),
                    source: "manual_id".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn list_sets_reports_card_and_owned_counts() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set_on(&c, "SOR", "Spark of Rebellion", "2024-03-08");
            testkit::simple(&c, "SOR", "1", "Luke Skywalker");
            testkit::simple(&c, "SOR", "2", "Millennium Falcon");
            // A second printing of card 1 is another printing, not another card.
            testkit::foil_of(&c, "SOR", "1");
        }
        let mut conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        own(&mut conn, &["SOR-1-normal"]);

        let sets = list_sets(&conn).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].set_code, "SOR");
        assert_eq!(sets[0].name, "Spark of Rebellion");
        assert_eq!(sets[0].release_date.as_deref(), Some("2024-03-08"));
        assert_eq!(sets[0].total_cards, 2);
        assert_eq!(sets[0].owned_cards, 1);
        assert_eq!(sets[0].total_printings, 3);
        assert_eq!(sets[0].owned_printings, 1);
    }

    #[test]
    fn list_sets_groups_by_category_main_expansions_first() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set_on(&c, "JTLP", "Jump to Lightspeed Weekly Play", "2025-04-01");
            testkit::set_on(&c, "SOR", "Spark of Rebellion", "2024-03-08");
            testkit::set_on(&c, "SHD", "Shadows of the Galaxy", "2024-07-12");
            testkit::set_on(&c, "ZZZ", "Unmapped", "2026-01-01");
            for code in ["JTLP", "SOR", "SHD"] {
                let cat = dumpstar_catalog::set_categories::category_for(code).unwrap();
                c.execute(
                    "UPDATE sets SET category = ?2 WHERE set_code = ?1",
                    rusqlite::params![code, cat],
                )
                .unwrap();
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let sets = list_sets(&conn).unwrap();
        let got: Vec<(&str, Option<&str>)> = sets
            .iter()
            .map(|s| (s.set_code.as_str(), s.category.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("SHD", Some("Main Expansions")),
                ("SOR", Some("Main Expansions")),
                ("JTLP", Some("Weekly Play Promos")),
                ("ZZZ", None),
            ]
        );
    }

    #[test]
    fn list_sets_orders_newest_release_first() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set_on(&c, "SOR", "Spark of Rebellion", "2024-03-08");
            testkit::set_on(&c, "SHD", "Shadows of the Galaxy", "2024-07-12");
            testkit::set_on(&c, "TWI", "Twilight of the Republic", "2024-11-08");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let codes: Vec<String> = list_sets(&conn)
            .unwrap()
            .into_iter()
            .map(|s| s.set_code)
            .collect();
        assert_eq!(codes, ["TWI", "SHD", "SOR"]);
    }

    #[test]
    fn analytics_breaks_down_completion_value_and_rarity() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            // The rarity split reads rank/grp off the `rarities` table;
            // the server reconciles it at startup, so a test that wants a
            // realistic split has to seed it too.
            crate::search_meta::reconcile(&mut c).unwrap();
            // Three cards: two Common, one Rare, priced $1, $2, $3.
            for (n, name, rarity_name, price) in [
                ("1", "Luke Skywalker", "Common", 1.0),
                ("2", "Millennium Falcon", "Common", 2.0),
                ("3", "Vanquish", "Rare", 3.0),
            ] {
                let pid = testkit::simple(&c, "SOR", n, name);
                rarity(&c, &pid, rarity_name);
                testkit::price(&c, &pid, "2026-05-18", price);
            }
        }
        let mut conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        // Own card 1 ($1) twice and card 3 ($3) once.
        own(&mut conn, &["SOR-1-normal", "SOR-1-normal", "SOR-3-normal"]);

        let a = analytics(&conn, "SOR").unwrap().unwrap();
        assert_eq!(a.total_cards, 3);
        assert_eq!(a.owned_cards, 2); // cards 1 and 3
        assert_eq!(a.total_printings, 3);
        assert_eq!(a.owned_printings, 2);
        assert_eq!(a.market_value, 6.0); // 1 + 2 + 3
        // Three physical copies → owned_value sums duplicates ($1+$1+$3),
        // owned_value_unique sums one-of-each owned printing ($1+$3).
        assert_eq!(a.owned_copies, 3);
        assert_eq!(a.owned_value, 5.0);
        assert_eq!(a.owned_value_unique, 4.0);

        // Per-card copy counts in collector-number order.
        assert_eq!(a.copy_counts.len(), 3);
        assert_eq!(a.copy_counts[0].number, "1");
        assert_eq!(a.copy_counts[0].sort_key, 1);
        assert_eq!(a.copy_counts[0].copies, 2);
        assert_eq!(a.copy_counts[1].number, "2");
        assert_eq!(a.copy_counts[1].copies, 0);
        assert_eq!(a.copy_counts[2].number, "3");
        assert_eq!(a.copy_counts[2].copies, 1);

        let common = a.rarities.iter().find(|r| r.rarity == "Common").unwrap();
        assert_eq!(common.total_cards, 2);
        assert_eq!(common.owned_cards, 1);
        let rare = a.rarities.iter().find(|r| r.rarity == "Rare").unwrap();
        assert_eq!(rare.total_cards, 1);
        assert_eq!(rare.owned_cards, 1);

        // The split carries the catalog's typology so the stats page can
        // order and colour it without a rarity map of its own.
        assert_eq!(common.grp.as_deref(), Some("common"));
        assert_eq!(rare.grp.as_deref(), Some("rare"));
        assert!(common.rank < rare.rank, "curated order, not alphabetical");
        assert_eq!(
            a.rarities
                .iter()
                .map(|r| r.rarity.as_str())
                .collect::<Vec<_>>(),
            ["Common", "Rare"],
            "rows arrive pre-sorted by rank"
        );
        // The histogram colours by group, so every column carries one.
        assert_eq!(a.copy_counts[0].rarity.as_deref(), Some("Common"));
        assert_eq!(a.copy_counts[0].rarity_grp.as_deref(), Some("common"));

        assert!(analytics(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn rarity_split_folds_upstream_spelling_onto_one_canonical_tier() {
        // Upstream ships the same tier two ways — "Legendary" from the
        // official API and "LEGENDARY" from elsewhere. They are one rarity,
        // and the split must say so: one row, both cards, the catalog's
        // spelling.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            crate::search_meta::reconcile(&mut c).unwrap();
            for (n, spelling) in [("1", "Legendary"), ("2", "LEGENDARY")] {
                let pid = testkit::simple(&c, "SHD", n, "Card");
                rarity(&c, &pid, spelling);
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();

        let a = analytics(&conn, "SHD").unwrap().unwrap();
        assert_eq!(a.rarities.len(), 1, "one tier, not two spellings");
        assert_eq!(a.rarities[0].rarity, "Legendary");
        assert_eq!(a.rarities[0].total_cards, 2);
        assert_eq!(a.rarities[0].grp.as_deref(), Some("legendary"));
        // The histogram gets the canonical spelling on both columns too.
        assert_eq!(a.copy_counts[1].rarity.as_deref(), Some("Legendary"));
        assert_eq!(a.copy_counts[1].rarity_grp.as_deref(), Some("legendary"));
    }

    #[test]
    fn unranked_rarity_keeps_its_string_and_sorts_last() {
        // A tier missing from data/rarities.json is a gap in the seed, not
        // a reason to drop the cards: it keeps its raw label and falls in
        // behind every ranked tier.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            crate::search_meta::reconcile(&mut c).unwrap();
            for (n, r) in [("1", "Blorbo Rare"), ("2", "Common")] {
                let pid = testkit::simple(&c, "ZZ1", n, "Card");
                rarity(&c, &pid, r);
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();

        let a = analytics(&conn, "ZZ1").unwrap().unwrap();
        assert_eq!(
            a.rarities
                .iter()
                .map(|r| r.rarity.as_str())
                .collect::<Vec<_>>(),
            ["Common", "Blorbo Rare"]
        );
        let unknown = &a.rarities[1];
        assert_eq!(unknown.rank, crate::search_meta::UNRANKED_RARITY);
        assert_eq!(unknown.grp, None);
    }

    #[test]
    fn rarity_and_number_come_from_the_standard_printing_of_each_card() {
        // A card with a Hyperspace printing at its own collector number is
        // still one card in the set: its histogram column is the Standard
        // printing's number and rarity, and copies of every printing add up.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            crate::search_meta::reconcile(&mut c).unwrap();
            let pid = testkit::simple(&c, "SOR", "5", "Luke Skywalker");
            rarity(&c, &pid, "Legendary");
            testkit::printing(
                &c,
                "SOR-5-hyper",
                "SOR-5",
                "SOR",
                "262",
                "Hyperspace",
                "Normal",
            );
            rarity(&c, "SOR-5-hyper", "Special");
        }
        let mut conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        own(&mut conn, &["SOR-5-normal", "SOR-5-hyper"]);

        let a = analytics(&conn, "SOR").unwrap().unwrap();
        assert_eq!(a.total_cards, 1);
        assert_eq!(a.total_printings, 2);
        assert_eq!(a.copy_counts.len(), 1);
        assert_eq!(a.copy_counts[0].number, "5");
        assert_eq!(a.copy_counts[0].copies, 2);
        assert_eq!(a.rarities.len(), 1);
        assert_eq!(a.rarities[0].rarity, "Legendary");
        assert_eq!(a.rarities[0].owned_cards, 1);
    }

    #[test]
    fn market_value_takes_cheapest_printing_per_card_not_sum_of_printings() {
        // A card printed as Standard, Hyperspace and Showcase must contribute
        // only its CHEAPEST printing to the full-set value — not the sum of
        // all three. Otherwise a set's "Full set" figure is dominated by the
        // Showcase.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "5", "Luke Skywalker");
            testkit::printing(
                &c,
                "SOR-5-hyper",
                "SOR-5",
                "SOR",
                "262",
                "Hyperspace",
                "Normal",
            );
            testkit::printing(
                &c,
                "SOR-5-show",
                "SOR-5",
                "SOR",
                "500",
                "Showcase",
                "Normal",
            );
            testkit::price(&c, "SOR-5-normal", "2026-06-01", 3.0);
            testkit::price(&c, "SOR-5-hyper", "2026-06-01", 8.0);
            testkit::price(&c, "SOR-5-show", "2026-06-01", 100.0);
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();

        let a = analytics(&conn, "SOR").unwrap().unwrap();
        assert_eq!(a.total_printings, 3);
        // Cheapest printing only, not 3 + 8 + 100 = 111.
        assert_eq!(a.market_value, 3.0);
    }
}
