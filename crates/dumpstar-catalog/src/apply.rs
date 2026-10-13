//! The apply step: parse landed bytes and fold them into the catalog.
//!
//! Idempotent at two levels. A payload whose hash is already in
//! `applied_payloads` is skipped outright. A payload with a *new* hash but
//! identical content (the same catalog re-serialised) still writes nothing,
//! because every upsert is guarded by "only when a column differs" and prices
//! are insert-if-absent. The report counts rows actually written.

use rusqlite::{Transaction, params};

use crate::Catalog;
use crate::error::{CatalogError, Result};
use crate::landing::Provenance;
use crate::record::{CatalogPayload, FORMAT};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// The payload's hash was already applied; nothing was parsed.
    pub already_applied: bool,
    pub sets: usize,
    pub cards: usize,
    pub printings: usize,
    pub crosswalk: usize,
    pub prices: usize,
    /// Price rows addressed by a tcgcsv (productId, subTypeName) no printing
    /// carries. Counted, not dropped silently: the unmatched-printing alarm
    /// reads this.
    pub unmatched_prices: usize,
    pub formats: usize,
    pub format_rules: usize,
    pub legality: usize,
}

impl ApplyReport {
    /// Whether the apply wrote no catalog row at all.
    pub fn is_noop(&self) -> bool {
        self.already_applied
            || (self.sets
                + self.cards
                + self.printings
                + self.crosswalk
                + self.prices
                + self.formats
                + self.format_rules
                + self.legality
                == 0)
    }
}

/// `UPDATE ... WHERE` guard: true when any listed column differs from the
/// incoming row, NULL-safe.
fn differs(cols: &[&str]) -> String {
    cols.iter()
        .map(|c| format!("{c} IS NOT excluded.{c}"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub fn apply(catalog: &mut Catalog, bytes: &[u8], prov: &Provenance) -> Result<ApplyReport> {
    let tx = catalog.conn.transaction()?;
    let seen = tx
        .prepare("SELECT 1 FROM applied_payloads WHERE payload_hash = ?1")?
        .exists([&prov.sha256])?;
    if seen {
        return Ok(ApplyReport {
            already_applied: true,
            ..Default::default()
        });
    }
    let payload: CatalogPayload = serde_json::from_slice(bytes)?;
    if payload.format != FORMAT {
        return Err(CatalogError::Format(payload.format));
    }
    let report = apply_payload(&tx, &payload)?;
    tx.execute(
        "INSERT OR IGNORE INTO applied_payloads (payload_hash, source, url) VALUES (?1, ?2, ?3)",
        params![prov.sha256, prov.source, prov.url],
    )?;
    tx.commit()?;
    Ok(report)
}

fn apply_payload(tx: &Transaction, p: &CatalogPayload) -> Result<ApplyReport> {
    let mut r = ApplyReport::default();

    let sql = format!(
        "INSERT INTO sets (set_code, name, release_date, sort_order) VALUES (?1,?2,?3,?4)
         ON CONFLICT(set_code) DO UPDATE SET name=excluded.name,
           release_date=excluded.release_date, sort_order=excluded.sort_order
         WHERE {}",
        differs(&["name", "release_date", "sort_order"])
    );
    for s in &p.sets {
        r.sets += tx.execute(
            &sql,
            params![s.set_code, s.name, s.release_date, s.sort_order],
        )?;
    }

    let cols = [
        "name",
        "subtitle",
        "card_type",
        "cost",
        "power",
        "hp",
        "aspects",
        "traits",
        "arenas",
        "rules_text",
    ];
    let sql = format!(
        "INSERT INTO cards (card_id, name, subtitle, card_type, cost, power, hp, aspects, traits, arenas, rules_text)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(card_id) DO UPDATE SET {}
         WHERE {}",
        cols.iter().map(|c| format!("{c}=excluded.{c}")).collect::<Vec<_>>().join(", "),
        differs(&cols)
    );
    for c in &p.cards {
        let changed = tx.execute(
            &sql,
            params![
                c.card_id,
                c.name,
                c.subtitle,
                c.card_type,
                c.cost,
                c.power,
                c.hp,
                serde_json::to_string(&c.aspects)?,
                serde_json::to_string(&c.traits)?,
                serde_json::to_string(&c.arenas)?,
                c.rules_text,
            ],
        )?;
        if changed > 0 {
            tx.execute("DELETE FROM card_search WHERE card_id = ?1", [&c.card_id])?;
            tx.execute(
                "INSERT INTO card_search (card_id, name, subtitle, rules_text, traits)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    c.card_id,
                    c.name,
                    c.subtitle,
                    c.rules_text,
                    c.traits.join(" ")
                ],
            )?;
        }
        r.cards += changed;
    }

    let cols = [
        "card_id",
        "set_code",
        "number",
        "variant",
        "finish",
        "rarity",
        "artist",
        "image_url",
    ];
    let sql = format!(
        "INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish, rarity, artist, image_url)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(printing_id) DO UPDATE SET {}
         WHERE {}",
        cols.iter().map(|c| format!("{c}=excluded.{c}")).collect::<Vec<_>>().join(", "),
        differs(&cols)
    );
    let xw = "INSERT INTO printing_source_id (source, source_id, sub_type_name, printing_id)
              VALUES (?1,?2,?3,?4)
              ON CONFLICT(source, source_id, sub_type_name) DO UPDATE SET printing_id=excluded.printing_id
              WHERE printing_id IS NOT excluded.printing_id";
    for pr in &p.printings {
        r.printings += tx.execute(
            &sql,
            params![
                pr.printing_id,
                pr.card_id,
                pr.set_code,
                pr.number,
                pr.variant,
                pr.finish,
                pr.rarity,
                pr.artist,
                pr.image_url
            ],
        )?;
        if let Some(uid) = &pr.official_card_uid {
            r.crosswalk += tx.execute(xw, params!["official", uid, pr.finish, pr.printing_id])?;
        }
        if let (Some(pid), Some(sub)) = (pr.tcgcsv_product_id, &pr.tcgcsv_sub_type_name) {
            r.crosswalk +=
                tx.execute(xw, params!["tcgcsv", pid.to_string(), sub, pr.printing_id])?;
        }
    }

    for f in &p.formats {
        r.formats += tx.execute(
            "INSERT INTO formats (format_id, name) VALUES (?1,?2)
             ON CONFLICT(format_id) DO UPDATE SET name=excluded.name WHERE name IS NOT excluded.name",
            params![f.format_id, f.name],
        )?;
        for (k, v) in &f.rules {
            r.format_rules += tx.execute(
                "INSERT INTO format_rules (format_id, rule_key, rule_value) VALUES (?1,?2,?3)
                 ON CONFLICT(format_id, rule_key) DO UPDATE SET rule_value=excluded.rule_value
                 WHERE rule_value IS NOT excluded.rule_value",
                params![f.format_id, k, v],
            )?;
        }
    }
    for l in &p.legality {
        r.legality += tx.execute(
            "INSERT INTO card_legality (format_id, card_id, status) VALUES (?1,?2,?3)
             ON CONFLICT(format_id, card_id) DO UPDATE SET status=excluded.status
             WHERE status IS NOT excluded.status",
            params![l.format_id, l.card_id, l.status],
        )?;
    }

    for pr in &p.prices {
        let printing: Option<String> = tx
            .query_row(
                "SELECT printing_id FROM printing_source_id
                 WHERE source='tcgcsv' AND source_id=?1 AND sub_type_name=?2",
                params![pr.tcgcsv_product_id.to_string(), pr.tcgcsv_sub_type_name],
                |row| row.get(0),
            )
            .ok();
        let Some(printing) = printing else {
            r.unmatched_prices += 1;
            continue;
        };
        r.prices += tx.execute(
            "INSERT OR IGNORE INTO prices (printing_id, source, observed_on, low, mid, high, market, direct_low)
             VALUES (?1,'tcgcsv',?2,?3,?4,?5,?6,?7)",
            params![printing, pr.observed_on, pr.low, pr.mid, pr.high, pr.market, pr.direct_low],
        )?;
    }
    Ok(r)
}
