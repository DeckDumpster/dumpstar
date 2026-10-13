//! Client for TCGCSV (`tcgcsv.com`) — the daily TCGplayer bulk dump.
//!
//! Provides set ("group") metadata, product lists and spot prices. The client
//! is parameterized over the TCGplayer category; Dumpstar reads
//! [`CATEGORY_STAR_WARS_UNLIMITED`]. No auth, no rate limit.

use std::sync::Arc;
use std::time::Duration;

use dumpstar_lake::{Dataset, PartFormat, RawLanding, Source};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{IngestError, Result};
use crate::landing::{self, Wire};

const BASE_URL: &str = "https://tcgcsv.com/tcgplayer";

/// TCGplayer's category id for Star Wars: Unlimited.
pub const CATEGORY_STAR_WARS_UNLIMITED: i64 = 79;

/// A TCGplayer "group" — roughly a set.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcgGroup {
    pub group_id: i64,
    pub name: String,
    pub abbreviation: Option<String>,
    pub published_on: Option<String>,
}

/// One `extendedData` entry on a product. Single cards carry a `Number`
/// entry here; sealed products do not.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtendedDatum {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// A TCGplayer product — either a single card or a sealed product.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcgProduct {
    pub product_id: i64,
    pub group_id: i64,
    pub name: String,
    pub image_url: Option<String>,
    pub url: Option<String>,
    /// TCGplayer's count of uploaded images for the product. Zero means
    /// the mechanically-generated `image_url` resolves to a 403 — common
    /// for newly-listed cards, promos and exclusives. We treat the URL as
    /// unusable when this is 0.
    #[serde(default)]
    pub image_count: i64,
    #[serde(default)]
    pub extended_data: Vec<ExtendedDatum>,
}

/// A spot price for one product + printing sub-type.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcgPrice {
    pub product_id: i64,
    pub sub_type_name: Option<String>,
    pub low_price: Option<f64>,
    pub mid_price: Option<f64>,
    pub high_price: Option<f64>,
    pub market_price: Option<f64>,
    pub direct_low_price: Option<f64>,
}

/// Extract and deserialize the `results` array from a TCGCSV envelope.
fn parse_results<T: DeserializeOwned>(envelope: &Value) -> Result<Vec<T>> {
    let arr = envelope
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| IngestError::BadResponse("TCGCSV envelope missing 'results'".into()))?;
    arr.iter()
        .map(|v| serde_json::from_value(v.clone()).map_err(IngestError::from))
        .collect()
}

/// Whether a product is a single card (it carries a `Number` extendedData
/// entry). Everything else is treated as a sealed product.
pub fn is_single_card(product: &TcgProduct) -> bool {
    product
        .extended_data
        .iter()
        .any(|e| e.name.eq_ignore_ascii_case("Number"))
}


/// Normalize a collector number so the catalog and TCGCSV agree.
///
/// pokemontcg.io stores bare numbers (`"6"`, `"H1"`, `"SWSH001"`) while
/// TCGCSV's `extendedData` "Number" carries the printed form, which for
/// modern sets is zero-padded and suffixed with the set total
/// (`"006/165"`, `"H01/H32"`). Linking by the raw strings matches almost
/// nothing. Normalization drops the `/total` suffix and collapses every
/// run of digits to its integer value, so both sides reduce to the same
/// token: `"006/165"` → `"6"`, `"H01/H32"` → `"h1"`, `"SWSH001"` →
/// `"swsh1"`. Applied identically to both sides it is order-preserving and
/// idempotent.
pub fn normalize_collector_number(raw: &str) -> String {
    let first = raw
        .split('/')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    let mut out = String::with_capacity(first.len());
    let mut digits = String::new();
    for ch in first.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            if !digits.is_empty() {
                out.push_str(&digits.parse::<u64>().unwrap_or(0).to_string());
                digits.clear();
            }
            out.push(ch);
        }
    }
    if !digits.is_empty() {
        out.push_str(&digits.parse::<u64>().unwrap_or(0).to_string());
    }
    out
}

/// A blocking client for the TCGCSV endpoints of one TCGplayer category.
pub struct TcgcsvClient {
    http: reqwest::blocking::Client,
    category_id: i64,
    wire: Wire,
    base_url: String,
}

impl TcgcsvClient {
    /// A client for Star Wars: Unlimited ([`CATEGORY_STAR_WARS_UNLIMITED`]).
    pub fn new() -> Result<Self> {
        Self::for_category(CATEGORY_STAR_WARS_UNLIMITED)
    }

    /// A client for an arbitrary TCGplayer category.
    pub fn for_category(category_id: i64) -> Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .user_agent("dumpstar/0.1 (+cache-population)")
                .timeout(Duration::from_secs(60))
                .build()?,
            category_id,
            wire: Wire::default(),
            base_url: crate::upstream::base_url(crate::upstream::ENV_TCGCSV_BASE_URL, BASE_URL),
        })
    }

    /// Land every response this client receives in `landing`.
    ///
    /// Without this the client behaves exactly as it did before the landing
    /// zone existed.
    pub fn landing_in(mut self, landing: Arc<RawLanding>) -> Self {
        self.wire = self.wire.landing_in(landing);
        self
    }

    /// Answer every request from `wire` — a landing zone to write through, a
    /// replay source to read from, or neither.
    pub fn on_wire(mut self, wire: Wire) -> Self {
        self.wire = wire;
        self
    }

    /// Retry on this budget instead of the environment's. Test-tier — see
    /// [`crate::retry`]; a real run reads `DUMPSTAR_HTTP_RETRY_*`.
    pub fn retry(mut self, retry: crate::retry::RetryPolicy) -> Self {
        self.wire = self.wire.retrying(retry);
        self
    }

    /// Point the client at a different origin. Test-tier only — it is how
    /// the landing path is driven against a local server instead of
    /// tcgcsv.com.
    pub fn base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    /// The TCGplayer category this client reads.
    pub fn category_id(&self) -> i64 {
        self.category_id
    }

    fn get(&self, path: &str, dataset: Dataset) -> Result<Value> {
        // A replayed response reaches no upstream, so there is nothing to be
        // polite to — and a sleep per request is pure delay.
        if !self.wire.is_replaying() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let category = self.category_id;
        let base = &self.base_url;
        let body = landing::fetch_bytes(
            &self.http,
            self.http.get(format!("{base}/{category}{path}")),
            &self.wire,
            Source::Tcgcsv,
            dataset,
            PartFormat::Json,
        )?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// Every group (set) in this client's category.
    pub fn fetch_groups(&self) -> Result<Vec<TcgGroup>> {
        parse_results(&self.get("/groups", Dataset::Groups)?)
    }

    /// Every product (cards + sealed) in a group.
    pub fn fetch_products(&self, group_id: i64) -> Result<Vec<TcgProduct>> {
        parse_results(&self.get(&format!("/{group_id}/products"), Dataset::Products)?)
    }

    /// Every spot price in a group.
    pub fn fetch_prices(&self, group_id: i64) -> Result<Vec<TcgPrice>> {
        parse_results(&self.get(&format!("/{group_id}/prices"), Dataset::Prices)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_groups_envelope() {
        let env: Value = serde_json::from_str(
            r#"{"success":true,"results":[
                 {"groupId":23237,"name":"151","abbreviation":"MEW",
                  "publishedOn":"2023-09-22T00:00:00"}]}"#,
        )
        .unwrap();
        let groups: Vec<TcgGroup> = parse_results(&env).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_id, 23237);
        assert_eq!(groups[0].abbreviation.as_deref(), Some("MEW"));
    }

    #[test]
    fn single_card_vs_sealed() {
        let card = TcgProduct {
            product_id: 1,
            group_id: 1,
            name: "Charizard ex".into(),
            image_url: None,
            url: None,
            image_count: 0,
            extended_data: vec![ExtendedDatum {
                name: "Number".into(),
                value: "6".into(),
            }],
        };
        let sealed = TcgProduct {
            product_id: 2,
            group_id: 1,
            name: "151 Elite Trainer Box".into(),
            image_url: None,
            url: None,
            image_count: 0,
            extended_data: vec![],
        };
        assert!(is_single_card(&card));
        assert!(!is_single_card(&sealed));
    }

    #[test]
    fn normalize_collector_number_reduces_both_sides() {
        // Modern sets: zero-padded, suffixed with the set total.
        assert_eq!(normalize_collector_number("006/165"), "6");
        assert_eq!(normalize_collector_number("170/165"), "170"); // secret rare
        // Catalog bare form normalizes to the same token.
        assert_eq!(normalize_collector_number("6"), "6");
        // Holo subsets: "H01/H32" must reduce to the catalog's "H1".
        assert_eq!(normalize_collector_number("H01/H32"), "h1");
        assert_eq!(normalize_collector_number("H1"), "h1");
        // Promo namespaces: padded digits inside an alpha prefix.
        assert_eq!(normalize_collector_number("SWSH001"), "swsh1");
        assert_eq!(
            normalize_collector_number("SWSH001"),
            normalize_collector_number("swsh1")
        );
        // Idempotent — re-normalizing changes nothing.
        let once = normalize_collector_number("009/165");
        assert_eq!(normalize_collector_number(&once), once);
    }
}
