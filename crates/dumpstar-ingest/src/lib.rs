//! `dumpstar-ingest` — upstream catalog ingestion for Dumpstar.
//!
//! Cache-population only: the TCGCSV client and its raw landing. Runtime card
//! lookups never touch the network — see `architecture/CARD_DATA_ACCESS.md`.

mod error;
pub mod landing;
pub mod retry;
pub mod tcgcsv;
/// The in-process fake upstream, behind the `test-support` feature — see
/// [`test_upstream`]. It lives in the library rather than in `tests/` because
/// `dumpstar-cli`'s raw-coverage gate drives a whole acquisition phase, whose
/// clients it never constructs and so cannot point anywhere itself.
#[cfg(any(test, feature = "test-support"))]
pub mod test_upstream;
pub mod upstream;

pub use error::{IngestError, Result};
