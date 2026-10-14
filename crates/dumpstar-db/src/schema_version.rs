//! The gate on every database's `PRAGMA user_version` is the kit's
//! (`dumpster-kit-tenancy::schema_version`); the versions Dumpstar understands
//! are in [`crate::app`].

pub use crate::app::{REGISTRY as Registry, SHARED as Shared, USER as User};
pub use dumpster_kit_tenancy::schema_version::{
    gate, gate_attached, stamp, version, version_of, version_of_file,
};
pub use dumpster_kit_tenancy::{SchemaSpec, SchemaState};
