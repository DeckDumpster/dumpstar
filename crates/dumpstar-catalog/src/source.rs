use crate::error::Result;

/// One fetch's response, exactly as received.
#[derive(Debug, Clone)]
pub struct RawPayload {
    /// Short stable name of the upstream: 'fake', 'official', 'tcgcsv'.
    pub source: String,
    pub url: String,
    pub bytes: Vec<u8>,
}

/// Anything that can produce raw catalog payloads. Fetching is its whole job:
/// it does not parse, land or apply.
pub trait CatalogSource {
    fn fetch(&self) -> Result<Vec<RawPayload>>;
}

/// The hand-written fake SWU catalog (`fake/swu-fake.json`): two sets, three
/// cards, every printing kind, two days of prices.
pub struct FakeSource;

pub const FAKE_CATALOG: &str = include_str!("../fake/swu-fake.json");
pub const FAKE_URL: &str = "fake://swu/catalog";

impl CatalogSource for FakeSource {
    fn fetch(&self) -> Result<Vec<RawPayload>> {
        Ok(vec![RawPayload {
            source: "fake".into(),
            url: FAKE_URL.into(),
            bytes: FAKE_CATALOG.as_bytes().to_vec(),
        }])
    }
}
