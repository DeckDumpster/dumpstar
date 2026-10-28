//! Star Wars: Unlimited as a [`dumpster_kit_catalog::Game`].
//!
//! The kit owns raw landing, the payload ledger, sync_state, alarms, prices, the tcgcsv
//! crosswalk store and sealed products; this module supplies what is SWU's own: the schema of
//! cards, printings and formats, the official-API parse and apply, the crosswalk rule and the
//! search vocabulary. The pre-kit `Catalog` / `apply` path stays until the readers move over.

use dumpster_kit_catalog::{
    ApplyReport, Error, FinishPlacement, Game, Match, Result, SearchTerm, Source, TermKind,
    Vocabulary,
};
use dumpster_kit_tcgcsv::{Price, Product};
use rusqlite::Transaction;

use crate::record::CatalogPayload;
use crate::swu::{OFFICIAL_BASE, SRC_OFFICIAL, TCGCSV_BASE};

/// TCGplayer's category for Star Wars: Unlimited.
pub const TCGCSV_CATEGORY: i64 = 79;

/// The kit-source name of the tcgcsv feed; the kit ingests it through [`Game::crosswalk`].
pub const SOURCE_TCGCSV: &str = "tcgcsv";

pub struct Swu;

impl Swu {
    pub const SCHEMA_SQL: &'static str = include_str!("schema_game.sql");
}

fn changes(report: &mut ApplyReport, r: &crate::apply::ApplyReport) {
    for (table, n) in [
        ("sets", r.sets),
        ("cards", r.cards),
        ("printings", r.printings),
        ("printing_source_id", r.crosswalk),
        ("rarity_marks", r.rarities),
        ("formats", r.formats),
        ("format_rules", r.format_rules),
        ("card_legality", r.legality),
    ] {
        report.updated(table, n as u64);
    }
}

impl Game for Swu {
    const ID: &'static str = "swu";
    /// 1: schema.sql without the kit's tables.
    const SCHEMA_VERSION: u32 = 1;
    const SCHEMA: &'static str = Swu::SCHEMA_SQL;
    const FINISH: FinishPlacement = FinishPlacement::OnPrinting;
    type Payload = CatalogPayload;

    fn sources() -> Vec<Source> {
        vec![
            Source {
                name: SRC_OFFICIAL.into(),
                location: format!("{OFFICIAL_BASE}/api/cards"),
            },
            Source {
                name: SOURCE_TCGCSV.into(),
                location: format!("{TCGCSV_BASE}/tcgplayer/{TCGCSV_CATEGORY}"),
            },
        ]
    }

    /// The official feed's card pages; the tcgcsv source carries no catalog rows (the kit
    /// ingests it through the crosswalk), so it parses to an empty payload.
    fn parse(source: &str, raw: &[u8]) -> Result<CatalogPayload> {
        match source {
            SRC_OFFICIAL => crate::official::parse_page(raw)
                .map(|p| p.payload)
                .map_err(|e| Error::Parse {
                    source_name: source.into(),
                    message: e.to_string(),
                }),
            SOURCE_TCGCSV => Ok(CatalogPayload::empty()),
            other => Err(Error::Parse {
                source_name: other.into(),
                message: "not a source of swu".into(),
            }),
        }
    }

    fn apply(tx: &Transaction<'_>, payload: &CatalogPayload) -> Result<ApplyReport> {
        let r = crate::apply::apply_payload_in(tx, payload, true)
            .map_err(|e| Error::Apply(e.to_string()))?;
        let mut report = ApplyReport::default();
        changes(&mut report, &r);
        Ok(report)
    }

    /// A numberless product is sealed. A numbered one names a printing the official feed owns
    /// (its id is a cardUid, which no tcgcsv field carries) and this function has no catalog to
    /// look it up in, so it is left unmatched and alarmed (sp-dilrmu.4 notes).
    fn crosswalk(product: &Product, _price: Option<&Price>) -> Match {
        match &product.number {
            None => Match::Sealed {
                sealed_id: format!("tcgcsv:{}", product.product_id),
            },
            Some(n) => Match::Unmatched {
                reason: format!(
                    "printing {n} of group {} needs the catalog",
                    product.group_id
                ),
            },
        }
    }

    fn search_terms() -> Vocabulary {
        use TermKind::{Number, Text};
        Vocabulary::new(
            "c.name",
            vec![
                SearchTerm::new("title", Text, "c.name", "card name").alias("name"),
                SearchTerm::new("subtitle", Text, "c.subtitle", "card subtitle").alias("sub"),
                SearchTerm::new("type", Text, "c.card_type", "Leader, Unit, Event...").alias("t"),
                SearchTerm::new("aspect", Text, "c.aspects", "aspect").alias("a"),
                SearchTerm::new("arena", Text, "c.arenas", "Ground or Space"),
                SearchTerm::new("trait", Text, "c.traits", "trait"),
                SearchTerm::new("rarity", Text, "p.rarity", "rarity").alias("r"),
                SearchTerm::new("set", Text, "p.set_code", "set code").alias("s"),
                SearchTerm::new("variant", Text, "p.variant", "Standard, Hyperspace...").alias("v"),
                SearchTerm::new("finish", Text, "p.finish", "Normal or Foil").alias("f"),
                SearchTerm::new("cost", Number, "c.cost", "resource cost").alias("c"),
                SearchTerm::new("power", Number, "c.power", "power"),
                SearchTerm::new("hp", Number, "c.hp", "hit points"),
            ],
        )
        .expect("valid vocabulary")
    }

    fn tcgcsv_category() -> i64 {
        TCGCSV_CATEGORY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn schema_runs_after_the_kits_ddl() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(dumpster_kit_catalog::KIT_DDL).unwrap();
        conn.execute_batch(Swu::SCHEMA).unwrap();
    }

    #[test]
    fn vocabulary_is_valid_and_sources_are_official_and_tcgcsv() {
        let _ = Swu::search_terms();
        let names: Vec<_> = Swu::sources().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["official", "tcgcsv"]);
    }

    #[test]
    fn empty_payload_applies_to_nothing() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(dumpster_kit_catalog::KIT_DDL).unwrap();
        conn.execute_batch(Swu::SCHEMA).unwrap();
        let tx = conn.transaction().unwrap();
        let r = Swu::apply(&tx, &CatalogPayload::empty()).unwrap();
        assert_eq!(r.total(), 0);
    }
}
