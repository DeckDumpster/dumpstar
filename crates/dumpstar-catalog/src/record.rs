//! The source-neutral catalog payload. An adapter for a real upstream maps
//! that upstream's responses onto this; the apply step reads nothing else.

use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "dumpstar-swu-catalog/1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogPayload {
    pub format: String,
    #[serde(default)]
    pub sets: Vec<SetRecord>,
    #[serde(default)]
    pub cards: Vec<CardRecord>,
    #[serde(default)]
    pub printings: Vec<PrintingRecord>,
    #[serde(default)]
    pub rarities: Vec<RarityRecord>,
    #[serde(default)]
    pub prices: Vec<PriceRecord>,
    #[serde(default)]
    pub formats: Vec<FormatRecord>,
    #[serde(default)]
    pub legality: Vec<LegalityRecord>,
    #[serde(default)]
    pub alarms: Vec<AlarmRecord>,
}

impl CatalogPayload {
    pub fn empty() -> Self {
        Self {
            format: FORMAT.into(),
            sets: vec![],
            cards: vec![],
            printings: vec![],
            rarities: vec![],
            prices: vec![],
            formats: vec![],
            legality: vec![],
            alarms: vec![],
        }
    }
}

/// A condition the operator must hear about. `severity` is 'alarm' (fails the
/// check) or 'info' (counted, never silent, but not paged).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct AlarmRecord {
    pub kind: String,
    pub key: String,
    pub severity: String,
    pub detail: String,
}

impl AlarmRecord {
    pub fn alarm(kind: &str, key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(kind, key, "alarm", detail)
    }
    pub fn info(kind: &str, key: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(kind, key, "info", detail)
    }
    fn new(kind: &str, key: impl Into<String>, sev: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            key: key.into(),
            severity: sev.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRecord {
    pub set_code: String,
    pub name: String,
    pub release_date: Option<String>,
    #[serde(default)]
    pub sort_order: i64,
    /// SWU product category; `None` for a set the declared mapping lacks.
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardRecord {
    pub card_id: String,
    pub name: String,
    pub subtitle: Option<String>,
    pub card_type: Option<String>,
    pub cost: Option<i64>,
    pub power: Option<i64>,
    pub hp: Option<i64>,
    #[serde(default)]
    pub aspects: Vec<String>,
    /// The record was read from the card's Standard non-foil entry. The
    /// official API lists a card's aspects in printed order only there; every
    /// other entry lists them by aspect id, so such a record never reorders
    /// aspects already stored (sp-mmro2r).
    #[serde(default)]
    pub from_standard: bool,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default)]
    pub arenas: Vec<String>,
    pub rules_text: Option<String>,
    /// The card's keywords, with the value the keyword carries (Raid 2).
    #[serde(default)]
    pub keywords: Vec<KeywordRecord>,
    /// A unique card: only one copy of it may be in play at a time.
    #[serde(default)]
    pub unique: bool,
    /// A leader's or base's epic action, when it has one.
    #[serde(default)]
    pub epic_action: Option<String>,
    /// The official styled text as HTML, icons included; see
    /// `card_text_styled` in schema.sql.
    #[serde(default)]
    pub rules_html: Option<String>,
}

/// One keyword on a card. `value` is the number the keyword carries (Raid 2,
/// Restore 1, Exploit 3) and `None` for one that has none (Sentinel, Bounty).
/// `granted` marks a keyword the card's text gives to another unit (an upgrade
/// that says "Attached unit gains Sentinel") rather than one the card has.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeywordRecord {
    pub name: String,
    #[serde(default)]
    pub value: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub granted: bool,
}

/// One official rarity: the mark (a one-letter character in a colour) and the
/// sort position the publisher gives it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RarityRecord {
    pub name: String,
    pub character: String,
    /// `#rrggbb`, as the publisher spells it.
    pub color: String,
    pub sort_value: i64,
}

/// One official printing, finish included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintingRecord {
    pub printing_id: String,
    pub card_id: String,
    pub set_code: String,
    pub number: String,
    pub variant: String,
    /// The official serial code's variant digits (`SS VV NNN`): what tells
    /// apart two printings that share set, number, display variant and finish.
    /// Empty when the source has no serial code.
    #[serde(default)]
    pub variant_code: String,
    pub finish: String,
    pub rarity: Option<String>,
    pub artist: Option<String>,
    pub image_url: Option<String>,
    /// The deployed side of a leader (the official `artBack`).
    #[serde(default)]
    pub back_image_url: Option<String>,
    /// The official `cardUid`. Foil and non-foil printings share it.
    pub official_card_uid: Option<String>,
    pub tcgcsv_product_id: Option<i64>,
    /// tcgcsv `subTypeName`: 'Normal' or 'Foil'.
    pub tcgcsv_sub_type_name: Option<String>,
}

/// One day's tcgcsv price row, addressed the way tcgcsv addresses it; the
/// apply step resolves it to a printing through the crosswalk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceRecord {
    pub tcgcsv_product_id: i64,
    pub tcgcsv_sub_type_name: String,
    pub observed_on: String,
    pub low: Option<f64>,
    pub mid: Option<f64>,
    pub high: Option<f64>,
    pub market: Option<f64>,
    pub direct_low: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatRecord {
    pub format_id: String,
    pub name: String,
    #[serde(default)]
    pub rules: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegalityRecord {
    pub format_id: String,
    pub card_id: String,
    pub status: String,
}
