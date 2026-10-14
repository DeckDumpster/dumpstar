//! The user registry is the kit's (`dumpster-kit-tenancy::registry`); only the opener binds
//! Dumpstar's data directory.

pub use dumpster_kit_tenancy::registry::*;

use rusqlite::Connection;

use crate::error::Result;

/// Open Dumpstar's `registry.sqlite`, creating it if absent.
pub fn open() -> Result<Connection> {
    Ok(dumpster_kit_tenancy::registry::open(crate::app::app())?)
}
