//! What makes the tenancy kernel Dumpstar's: the one [`AppPrefix`] it passes in
//! and the schema versions it understands. Everything else is the kit's.

use std::sync::OnceLock;

use dumpster_kit_tenancy::AppPrefix;
pub use dumpster_kit_tenancy::SchemaSpec;

/// Dumpstar's identity: `DUMPSTAR_HOME`, `DUMPSTAR_USER`, `DUMPSTAR_ACCESS_*`.
pub fn app() -> &'static AppPrefix {
    static APP: OnceLock<AppPrefix> = OnceLock::new();
    APP.get_or_init(|| AppPrefix::new("DUMPSTAR").expect("DUMPSTAR is a valid prefix"))
}

/// The immutable card catalog, `shared.sqlite`.
///
/// Version 3: sp-r7w5xi replaced the Pokémon card model with the Star Wars: Unlimited one. A
/// build that knows only the old shape would read the new file wrongly, so it is refused.
pub const SHARED: SchemaSpec = SchemaSpec {
    label: "shared catalog",
    version: 3,
};

/// A per-tenant collection, `tenants/<database_id>.sqlite`.
///
/// Version 2: pd-s4c2 moved `conditions` out of the catalog and into the collection, which a
/// build that does not know it would get wrong rather than merely miss.
pub const USER: SchemaSpec = SchemaSpec {
    label: "collection",
    version: 2,
};

/// The user registry, `registry.sqlite`. The kit owns its version.
pub use dumpster_kit_tenancy::schema_version::REGISTRY;
