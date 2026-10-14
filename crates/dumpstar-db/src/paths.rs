//! Dumpstar's data-directory layout: the kit's path functions with Dumpstar's
//! [`AppPrefix`] bound. The layout, the handle rule and the `database_id` rule
//! are the kit's (`dumpster-kit-tenancy::paths`); nothing here decides them.

use std::path::PathBuf;

use crate::app::app;
use crate::error::Result;
use dumpster_kit_tenancy::paths as kit;

pub use dumpster_kit_tenancy::paths::{
    HANDLE_RULE, TENANTS_DIR, tenant_db_file, validate_database_id, validate_tenant_name,
};

/// `$DUMPSTAR_HOME` if set, else `$HOME/.dumpstar`.
pub fn dumpstar_home() -> Result<PathBuf> {
    Ok(kit::home(app())?)
}

pub fn shared_db_path() -> Result<PathBuf> {
    Ok(kit::shared_db_path(app())?)
}

pub fn tenants_dir() -> Result<PathBuf> {
    Ok(kit::tenants_dir(app())?)
}

pub fn registry_db_path() -> Result<PathBuf> {
    Ok(kit::registry_db_path(app())?)
}

/// `$DUMPSTAR_USER` if set, else `collection`.
pub fn current_user() -> String {
    kit::current_user(app())
}

pub fn tenant_db_path(name: &str) -> Result<PathBuf> {
    Ok(kit::tenant_db_path(app(), name)?)
}

pub fn tenant_db_path_for_id(database_id: &str) -> Result<PathBuf> {
    Ok(kit::tenant_db_path_for_id(app(), database_id)?)
}

pub fn legacy_user_db_path(name: &str) -> Result<PathBuf> {
    Ok(kit::legacy_user_db_path(app(), name)?)
}

#[cfg(test)]
pub(crate) fn with_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
    kit::with_home(app(), f)
}
