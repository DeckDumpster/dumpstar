//! `dumpstar-swu` — the Star Wars: Unlimited catalog seam.
//!
//! A [`CatalogSource`] yields one raw payload per fetch; [`landing::land`]
//! writes it to disk with its URL, time and SHA-256 *before* anything parses
//! it; [`apply::apply`] then parses the landed bytes and folds them into
//! `shared.sqlite` idempotently. [`job::run_once`] is that whole sequence and is
//! what a systemd timer runs. No request handler applies a catalog:
//! [`Catalog::open_readonly`] is the only way in from the serving path, and the
//! file is opened with SQLite's read-only flag so a write fails in the engine
//! rather than by convention.
//!
//! Real upstreams (the official API, tcgcsv) are adapters over
//! [`CatalogSource`] that produce the [`record::CatalogPayload`] shape; the
//! hand-written [`source::FakeSource`] ships with the seam.

pub mod apply;
pub mod art;
mod catalog;
pub mod check;
pub mod crosswalk;
mod error;
pub mod fetch;
pub mod filing;
pub mod formats;
pub mod images;
pub mod job;
pub mod landing;
pub mod mechanics;
pub mod official;
pub mod record;
pub mod search;
pub mod set_categories;
pub mod source;
pub mod swu;
pub mod tcgcsv;

pub use catalog::{
    Catalog, LEGACY_ONLY_OBJECTS, LEGACY_SHAPE_DDL, SCHEMA, add_missing_columns, is_legacy,
    reset_if_legacy,
};
pub use error::{CatalogError, Result};
pub use source::{CatalogSource, FakeSource, RawPayload};
