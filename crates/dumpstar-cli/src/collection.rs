//! The one place a command asks which collection database it is working on.
//!
//! Every `dumpstar` subcommand that touches a collection — `serve`, `db`,
//! `export`, `import`, `data` — resolves it through here, so they cannot
//! disagree about what `$DUMPSTAR_USER` means. [`dumpstar_db::tenants::resolve`]
//! does the deciding; this adds the one thing a library should not, which is
//! saying out loud when a data directory is still on the pre-`pd-hqee` layout.
//!
//! That warning is not decoration. A handle-named database is served exactly
//! as it is — production must not need a migration to keep running — so the
//! *only* signal that the migration is still outstanding is this line.

use std::path::PathBuf;

use dumpstar_db::tenants::Collection;

/// The collection database for `$DUMPSTAR_USER`, warning on stderr if this data
/// directory has not been migrated onto opaque ids yet.
pub fn user_db() -> anyhow::Result<PathBuf> {
    Ok(resolve()?.path)
}

/// [`user_db`], keeping how it was resolved — for `serve`, which reports the
/// tenant it came up on.
pub fn resolve() -> anyhow::Result<Collection> {
    let handle = dumpstar_db::current_user();
    let collection = dumpstar_db::resolve_collection(&handle)?;
    if collection.is_unmigrated() {
        // stderr, not stdout: `dumpstar export --json -o -` writes a collection
        // to stdout, and a warning in the middle of it would corrupt a backup.
        eprintln!(
            "warning: tenant {handle:?} is served from {}, which is named by handle \
             rather than by an opaque database id.\n\
             \x20        Run `dumpstar tenant migrate` to move it onto one \
             (see deploy/TENANTS.md). Serving it as-is until you do.",
            collection.path.display()
        );
    }
    Ok(collection)
}
