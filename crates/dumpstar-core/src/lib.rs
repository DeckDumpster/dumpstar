//! `dumpstar-core` — domain types and pure logic for Dumpstar.
//!
//! No IO lives here. The collection search query language's parser lives in
//! `query` (the SQL compiler that consumes its AST is in `dumpstar-db`).

pub mod card;
pub mod deck;
pub mod import;
pub mod query;

pub use card::number_sort_key;

/// Crate version string, surfaced by the CLI and server.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
