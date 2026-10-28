//! Formats and their deckbuilding rules, as data.
//!
//! `data/formats.json` is the starting data, each format citing the section of
//! the retrieved Comprehensive Rules (`fixtures/swu/rules/`) it comes from.
//! [`Catalog::seed_formats`] writes it into `formats` / `format_rules`
//! idempotently; the readers below hand the checker in `dumpstar-core` what it
//! needs. Card legality (`card_legality`) arrives with catalog payloads — the
//! suspended list changes at every rotation and is not baked in here.

use std::collections::{BTreeMap, HashMap};

use dumpstar_core::deck::{CardFacts, FormatRules, Legality};
use rusqlite::{OptionalExtension, params};

use crate::Catalog;
use crate::error::{CatalogError, Result};
use crate::record::FormatRecord;

const FORMATS: &str = include_str!("../data/formats.json");

pub fn builtin_formats() -> Vec<FormatRecord> {
    serde_json::from_str(FORMATS).expect("data/formats.json is checked by a test")
}

impl Catalog {
    /// Upsert the builtin formats. Returns the number of rows changed; a
    /// second call returns 0.
    pub fn seed_formats(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for f in builtin_formats() {
            n += tx.execute(
                "INSERT INTO formats (format_id, name) VALUES (?1,?2)
                 ON CONFLICT(format_id) DO UPDATE SET name=excluded.name WHERE name IS NOT excluded.name",
                params![f.format_id, f.name],
            )?;
            for (k, v) in &f.rules {
                n += tx.execute(
                    "INSERT INTO format_rules (format_id, rule_key, rule_value) VALUES (?1,?2,?3)
                     ON CONFLICT(format_id, rule_key) DO UPDATE SET rule_value=excluded.rule_value
                     WHERE rule_value IS NOT excluded.rule_value",
                    params![f.format_id, k, v],
                )?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// The rules of `format_id`; `None` when no such format exists.
    pub fn format_rules(&self, format_id: &str) -> Result<Option<FormatRules>> {
        let known: Option<String> = self
            .conn
            .query_row(
                "SELECT format_id FROM formats WHERE format_id=?1",
                [format_id],
                |r| r.get(0),
            )
            .optional()?;
        if known.is_none() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT rule_key, rule_value FROM format_rules WHERE format_id=?1")?;
        let map: BTreeMap<String, String> = stmt
            .query_map([format_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        FormatRules::from_map(&map)
            .map(Some)
            .map_err(|e| CatalogError::Source(format!("format {format_id}: {e}")))
    }

    /// Facts for each of `card_ids` that exists in the catalog, with its
    /// standing in `format_id` (a card with no `card_legality` row is legal).
    pub fn card_facts(
        &self,
        format_id: &str,
        card_ids: &[&str],
    ) -> Result<HashMap<String, CardFacts>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.card_type, c.aspects, l.status
               FROM cards c
               LEFT JOIN card_legality l ON l.card_id = c.card_id AND l.format_id = ?2
              WHERE c.card_id = ?1",
        )?;
        let mut out = HashMap::new();
        for id in card_ids {
            let row = stmt
                .query_row(params![id, format_id], |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .optional()?;
            if let Some((ty, aspects, status)) = row {
                out.insert(
                    (*id).to_string(),
                    CardFacts {
                        card_type: ty.unwrap_or_default(),
                        aspects: serde_json::from_str(&aspects)?,
                        legality: status
                            .as_deref()
                            .and_then(Legality::parse)
                            .unwrap_or_default(),
                    },
                );
            }
        }
        Ok(out)
    }
}
