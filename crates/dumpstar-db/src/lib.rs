//! `dumpstar-db` — SQLite persistence for Dumpstar.
//!
//! Owns the shared/user database split (PLAN.md §3.1): a read-only shared
//! catalog `ATTACH`ed to a mutable per-user collection database, plus the
//! application-layer foreign-key checks against the catalog. The full
//! schema for each database lives in `schema_shared.sql` / `schema_user.sql`
//! and is re-applied idempotently on every open (dumpstar-luo).

pub mod app;
/// The set-art store reader, re-exported so the server reads art through the db crate.
pub use dumpstar_catalog::art;
pub mod batches;
pub mod binder;
pub mod binders;
pub mod cards;
pub mod catalog;
pub mod clock;
pub mod collection;
pub mod collectr_export;
pub mod conditions;
pub mod decks;
pub mod export;
pub mod import;
pub mod json_backup;
pub mod manual_prices;
pub mod natsort;
pub mod orders;
pub mod outbox;
pub mod prices;
pub mod raw_derivation;
pub mod registry;
pub mod schema_version;
pub mod sealed;
pub mod sealed_import;
pub mod search;
pub mod search_meta;
pub mod sets;
pub mod tenants;
pub mod unresolved;
pub mod user_printings;
pub mod value_history;
pub mod wishlist;

mod connection;
mod convergence;
mod error;
mod paths;
#[cfg(test)]
mod testkit;

pub use connection::{
    BUSY_TIMEOUT, WalCheckpoint, WalReclaim, attach_shared_readonly, checkpoint_truncate,
    connect_user, init_user_schema, open_registry, open_shared, open_shared_for_serving,
    open_shared_readonly, open_shared_with_patience, open_user, reclaim_catalog_wal, restore_db,
    snapshot_db, user_spec,
};
pub use error::{DbError, Result};
pub use paths::{
    HANDLE_RULE, TENANTS_DIR, current_user, dumpstar_home, legacy_user_db_path, registry_db_path,
    shared_db_path, tenant_db_file, tenant_db_path, tenant_db_path_for_id, tenants_dir,
    validate_database_id, validate_tenant_name,
};
/// Which collection single-tenant mode serves for a handle. Not in `paths`
/// because it is not a path calculation: it reads the user registry, and the
/// answer for a migrated data directory is a file whose name the handle does
/// not appear in. See [`tenants::resolve`].
pub use tenants::resolve as resolve_collection;
