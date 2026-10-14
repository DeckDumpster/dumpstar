//! `dumpstar-ship` — the shipper, as dumpstar binds it.
//!
//! The shipper itself (outbox → encrypted Parquet parts in the tenant zone, resumable,
//! gap-detecting; and the zone read-back) is `dumpster-kit-ship`: one implementation, used by
//! every site. What is dumpstar's is here, and it is small: the `DUMPSTAR` names the kit's
//! environment types are built from, the registry and tenant openers a [`ShipContext`] is made
//! of, and the `dumpstar-ship` operator binary over them. The triggers on dumpstar's holdings
//! tables are in `dumpstar-db`'s `schema_user.sql`.
//!
//! Offline, like `dumpstar-lake-derive`: it needs the tenant-zone credentials and the master
//! key, and nothing that serves a request may hold either.

use std::path::Path;

use dumpster_kit_keys::master::KeyEnv;
use dumpster_kit_lake::{LakeEnv, TenantZoneConfig};
use dumpster_kit_ship::{ShipContext, ShipError, Tenant};
use rusqlite::Connection;

pub use dumpster_kit_ship::{run, ship_all, ship_one, zone};

/// The names the lake reads, rooted at `DUMPSTAR`.
pub fn lake_env() -> LakeEnv {
    LakeEnv::new(dumpstar_db::app::app())
}

/// The names key custody reads, rooted at `DUMPSTAR`.
pub fn key_env() -> KeyEnv {
    KeyEnv::new(dumpstar_db::app::app())
}

/// The key-custody registry, opened through the kernel's one opener.
pub fn registry() -> Result<Connection, dumpster_kit_keys::KeyError> {
    dumpster_kit_keys::state::open(dumpstar_db::app::app())
}

/// A tenant as the kit's shipper wants it.
pub fn kit_tenant(t: &dumpstar_db::tenants::Tenant) -> Tenant {
    Tenant {
        user: t.user.clone(),
        path: t.path.clone(),
    }
}

fn open_tenant(path: &Path) -> Result<Connection, ShipError> {
    dumpstar_db::open_user(path).map_err(|e| match e {
        dumpstar_db::DbError::Sqlite(e) => ShipError::Sqlite(e),
        other => ShipError::Tenancy(dumpster_kit_tenancy::TenancyError::Env(other.to_string())),
    })
}

/// Everything `ship_all` / `ship_one` need, bound to dumpstar. The caller owns the borrowed
/// pieces; this only supplies the names and the tenant opener.
pub fn context<'a>(
    keys: &'a KeyEnv,
    zone: &'a dyn dumpster_kit_lake::ObjectStore,
    config: &'a TenantZoneConfig,
    registry: &'a Connection,
) -> ShipContext<'a> {
    let (crash_var, cli) = ShipContext::names(dumpstar_db::app::app());
    ShipContext {
        keys,
        zone,
        config,
        registry,
        open_tenant: &open_tenant,
        crash_var,
        cli,
    }
}
