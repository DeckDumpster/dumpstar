//! Dumpstar's transport for the kit's TCGCSV client.
//!
//! The URL layout, the typed group/product/price records and the
//! `last-updated.txt` short-circuit live in `dumpster-kit-tcgcsv`; this module
//! supplies what the kit leaves to its consumer: a [`Fetch`] that goes through
//! [`landing::fetch_bytes`], so retry, raw landing and replay apply to every
//! request, exactly as before.

use std::sync::Arc;
use std::time::Duration;

use dumpster_kit_lake::{Dataset, PartFormat, RawLanding, Source};
use dumpster_kit_tcgcsv::Fetch;
pub use dumpster_kit_tcgcsv::{Group, Price, Product};

use crate::error::Result;
use crate::landing::{self, Wire};

/// TCGplayer's category id for Star Wars: Unlimited.
pub const CATEGORY_STAR_WARS_UNLIMITED: i64 = 79;

/// A blocking client for the TCGCSV endpoints of one TCGplayer category.
pub struct TcgcsvClient {
    http: reqwest::blocking::Client,
    category_id: i64,
    wire: Wire,
    /// The origin; the kit appends `/tcgplayer/<category>/...`.
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
            base_url: crate::upstream::base_url(
                crate::upstream::ENV_TCGCSV_BASE_URL,
                dumpster_kit_tcgcsv::DEFAULT_BASE,
            ),
        })
    }

    /// Land every response this client receives in `landing`.
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

    /// Point the client at a different origin (no `/tcgplayer`). Test-tier only.
    pub fn base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    /// The TCGplayer category this client reads.
    pub fn category_id(&self) -> i64 {
        self.category_id
    }

    fn kit(&self) -> dumpster_kit_tcgcsv::TcgcsvClient<'_> {
        dumpster_kit_tcgcsv::TcgcsvClient::new(self, &self.base_url, self.category_id)
    }

    /// Every group (set) in this client's category.
    pub fn fetch_groups(&self) -> Result<Vec<Group>> {
        Ok(self.kit().fetch_groups()?)
    }

    /// Every product (cards + sealed) in a group.
    pub fn fetch_products(&self, group_id: i64) -> Result<Vec<Product>> {
        Ok(self.kit().fetch_products(group_id)?)
    }

    /// Every spot price in a group.
    pub fn fetch_prices(&self, group_id: i64) -> Result<Vec<Price>> {
        Ok(self.kit().fetch_prices(group_id)?)
    }
}

/// Which landed dataset a TCGCSV URL belongs to, by its last path segment.
fn dataset_of(url: &str) -> Option<Dataset> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('/').next()? {
        "groups" => Some(Dataset::Groups),
        "products" => Some(Dataset::Products),
        "prices" => Some(Dataset::Prices),
        _ => None,
    }
}

impl Fetch for TcgcsvClient {
    fn get(&self, url: &str) -> dumpster_kit_tcgcsv::Result<Vec<u8>> {
        let fail = |message: String| dumpster_kit_tcgcsv::Error::Fetch {
            url: url.to_string(),
            message,
        };
        let dataset = dataset_of(url)
            .ok_or_else(|| fail("not a landed TCGCSV dataset (groups/products/prices)".into()))?;
        // A replayed response reaches no upstream, so there is nothing to be
        // polite to — and a sleep per request is pure delay.
        if !self.wire.is_replaying() {
            std::thread::sleep(Duration::from_millis(50));
        }
        landing::fetch_bytes(
            &self.http,
            self.http.get(url),
            &self.wire,
            Source::Tcgcsv,
            dataset,
            PartFormat::Json,
        )
        .map_err(|e| fail(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datasets_are_read_from_the_last_segment() {
        assert_eq!(
            dataset_of("https://tcgcsv.com/tcgplayer/79/groups"),
            Some(Dataset::Groups)
        );
        assert_eq!(
            dataset_of("http://x/tcgplayer/79/17/products?a=b"),
            Some(Dataset::Products)
        );
        assert_eq!(
            dataset_of("http://x/tcgplayer/79/17/prices"),
            Some(Dataset::Prices)
        );
        assert_eq!(dataset_of("https://tcgcsv.com/last-updated.txt"), None);
    }
}
