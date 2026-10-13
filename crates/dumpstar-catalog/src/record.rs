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
    pub prices: Vec<PriceRecord>,
    #[serde(default)]
    pub formats: Vec<FormatRecord>,
    #[serde(default)]
    pub legality: Vec<LegalityRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRecord {
    pub set_code: String,
    pub name: String,
    pub release_date: Option<String>,
    #[serde(default)]
    pub sort_order: i64,
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
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default)]
    pub arenas: Vec<String>,
    pub rules_text: Option<String>,
}

/// One official printing, finish included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintingRecord {
    pub printing_id: String,
    pub card_id: String,
    pub set_code: String,
    pub number: String,
    pub variant: String,
    pub finish: String,
    pub rarity: Option<String>,
    pub artist: Option<String>,
    pub image_url: Option<String>,
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
