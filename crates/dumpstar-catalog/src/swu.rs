//! The two live upstream adapters over [`CatalogSource`]: the official API
//! (a catalog crawl, at most one a day, incremental by `updatedAt`) and tcgcsv
//! (prices, once per tcgcsv refresh). They only fetch: every response is
//! landed verbatim before [`crate::apply`] parses it.
//!
//! Each ends with a marker payload that advances its resume point. The apply
//! step runs payloads in order, so a crash half way leaves the marker unwritten
//! and the next run starts again from the old point.

use serde::{Deserialize, Serialize};

use crate::error::{CatalogError, Result};
use crate::fetch::Fetch;
use crate::official;
use crate::source::{CatalogSource, RawPayload};
use crate::tcgcsv::{self, CATEGORY};

pub const OFFICIAL_BASE: &str = "https://admin.starwarsunlimited.com";
pub const TCGCSV_BASE: &str = "https://tcgcsv.com";

pub const SRC_OFFICIAL: &str = "official";
pub const SRC_OFFICIAL_CRAWL: &str = "official-crawl";
pub const SRC_GROUPS: &str = "tcgcsv-groups";
pub const SRC_PRODUCTS: &str = "tcgcsv-products";
pub const SRC_PRICES: &str = "tcgcsv-prices";
pub const SRC_LAST_UPDATED: &str = "tcgcsv-last-updated";

pub const KEY_OFFICIAL_SINCE: &str = "official.updated_since";
pub const KEY_OFFICIAL_CRAWLED_ON: &str = "official.crawled_on";
pub const KEY_TCGCSV_LAST_UPDATED: &str = "tcgcsv.last_updated";

#[derive(Serialize, Deserialize)]
pub struct CrawlMarker {
    pub updated_since: Option<String>,
    pub crawled_on: String,
}

pub struct OfficialSource<'a> {
    pub http: &'a dyn Fetch,
    pub base: String,
    /// Resume point: the largest `updatedAt` a finished crawl saw.
    pub since: Option<String>,
    pub crawled_on: Option<String>,
    /// Today, UTC, `YYYY-MM-DD`.
    pub today: String,
}

impl OfficialSource<'_> {
    pub fn page_url(&self, page: u32) -> String {
        let mut u = format!(
            "{}/api/cards?locale=en&pagination[pageSize]={}&pagination[page]={page}&sort[0]=id:asc\
             &populate[0]=expansion&populate[1]=variantTypes&populate[2]=rarity",
            self.base.trim_end_matches('/'),
            official::PAGE_SIZE
        );
        if let Some(s) = &self.since {
            u.push_str(&format!("&filters[updatedAt][$gte]={s}"));
        }
        u
    }
}

impl CatalogSource for OfficialSource<'_> {
    fn fetch(&self) -> Result<Vec<RawPayload>> {
        if self.crawled_on.as_deref() == Some(self.today.as_str()) {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        let mut max = self.since.clone();
        let mut page = 1;
        loop {
            let url = self.page_url(page);
            let bytes = self.http.get(&url)?;
            let parsed = official::parse_page(&bytes)?;
            if let Some(u) = parsed.max_updated_at
                && max.as_ref().is_none_or(|m| &u > m)
            {
                max = Some(u);
            }
            let count = parsed.page_count;
            out.push(RawPayload {
                source: SRC_OFFICIAL.into(),
                url,
                bytes,
                as_of: None,
            });
            if page >= count {
                break;
            }
            page += 1;
        }
        let marker = CrawlMarker {
            updated_since: max,
            crawled_on: self.today.clone(),
        };
        out.push(RawPayload {
            source: SRC_OFFICIAL_CRAWL.into(),
            url: format!("{}/api/cards#crawl", self.base.trim_end_matches('/')),
            bytes: serde_json::to_vec(&marker)?,
            as_of: Some(self.today.clone()),
        });
        Ok(out)
    }
}

pub struct TcgcsvSource<'a> {
    pub http: &'a dyn Fetch,
    pub base: String,
    pub last_updated: Option<String>,
}

impl CatalogSource for TcgcsvSource<'_> {
    fn fetch(&self) -> Result<Vec<RawPayload>> {
        let base = self.base.trim_end_matches('/');
        let stamp_url = format!("{base}/last-updated.txt");
        let stamp_bytes = self.http.get(&stamp_url)?;
        let stamp = String::from_utf8_lossy(&stamp_bytes).trim().to_string();
        let day = tcgcsv::stamp_day(&stamp)
            .ok_or_else(|| CatalogError::Source(format!("last-updated.txt unreadable: {stamp:?}")))?
            .to_string();
        if self.last_updated.as_deref() == Some(stamp.as_str()) {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        let groups_url = format!("{base}/tcgplayer/{CATEGORY}/groups");
        let groups_bytes = self.http.get(&groups_url)?;
        let groups = tcgcsv::parse_groups(&groups_bytes)?;
        out.push(RawPayload {
            source: SRC_GROUPS.into(),
            url: groups_url,
            bytes: groups_bytes,
            as_of: Some(day.clone()),
        });
        for g in groups {
            for (source, leaf) in [(SRC_PRODUCTS, "products"), (SRC_PRICES, "prices")] {
                let url = format!("{base}/tcgplayer/{CATEGORY}/{}/{leaf}", g.group_id);
                out.push(RawPayload {
                    source: source.into(),
                    bytes: self.http.get(&url)?,
                    url,
                    as_of: Some(day.clone()),
                });
            }
        }
        out.push(RawPayload {
            source: SRC_LAST_UPDATED.into(),
            url: stamp_url,
            bytes: stamp_bytes,
            as_of: Some(day),
        });
        Ok(out)
    }
}
