//! The seam between the kit's URL-keyed replay and dumpstar's ingest.
//!
//! `dumpster_kit_lakehouse::replay::RawReplay` answers a URL out of a landed partition; this
//! adapts it to [`ReplaySource`], the trait `dumpstar-ingest`'s wire reads. The index, the
//! SHA-256 check and the diagnostics are the kit's; only the error type is ours.

use dumpstar_ingest::landing::ReplaySource;
use dumpstar_ingest::{IngestError, Result};
use dumpster_kit_lakehouse::replay::{RawReplay, ReplayError};

pub struct Replay(pub RawReplay);

impl Replay {
    pub fn urls(&self) -> usize {
        self.0.urls()
    }
}

fn ingest_error(e: ReplayError) -> IngestError {
    match e {
        ReplayError::Miss(text) => IngestError::BadResponse(text),
        ReplayError::Lake(e) => IngestError::Lake(e),
    }
}

impl ReplaySource for Replay {
    fn body(&self, url: &str) -> Result<Option<Vec<u8>>> {
        self.0.body(url).map_err(ingest_error)
    }

    fn missing(&self, url: &str) -> IngestError {
        ingest_error(self.0.missing(url))
    }
}
