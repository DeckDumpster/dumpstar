//! `dumpstar db` — database maintenance helpers.
//!
//! `snapshot` / `restore` give the UI test harness WAL-correct, dependency-free
//! per-test isolation. They replace the old in-container
//! `python3 sqlite3.backup()` + `cp` dance, which needed python3 the runtime
//! image doesn't ship (dumpstar-0g3) and whose `cp` restore was
//! WAL-unaware, leaking a prior test's writes across the isolation boundary
//! (dumpstar-lxm). Paths are resolved from `$DUMPSTAR_HOME` / `$DUMPSTAR_USER`,
//! the same as `dumpstar serve`, so `podman exec <ctr> dumpstar db snapshot` Just
//! Works against a running instance.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use dumpstar_db::{restore_db, shared_db_path, snapshot_db};

/// Arguments for `dumpstar db`.
#[derive(clap::Args)]
pub struct DbArgs {
    #[command(subcommand)]
    command: DbCommand,
}

#[derive(clap::Subcommand)]
enum DbCommand {
    /// Snapshot the collection (and shared catalog) to sibling `.bak` files.
    Snapshot,
    /// Restore the collection (and shared catalog) from their `.bak` files.
    Restore,
    /// Run read-only SQL against a tenant, with the shared catalog attached as the server has it.
    Query(SqlArgs),
    /// EXPLAIN QUERY PLAN for read-only SQL, then run it and print the wall time.
    Explain(SqlArgs),
}

#[derive(clap::Args)]
struct SqlArgs {
    /// `current`, a handle, or a database id.
    #[arg(long, default_value = "current")]
    tenant: String,
    /// The SQL to run.
    sql: String,
}

/// Execute `dumpstar db`.
pub fn run(args: DbArgs) -> anyhow::Result<()> {
    match args.command {
        DbCommand::Snapshot => snapshot(),
        DbCommand::Restore => restore(),
        DbCommand::Query(a) => {
            let conn = crate::ops::open_readonly(
                &crate::ops::tenant_path(&a.tenant)?,
                &shared_db_path()?,
            )?;
            let n = crate::ops::query(&conn, &a.sql, &mut std::io::stdout())?;
            eprintln!("{n} rows");
            Ok(())
        }
        DbCommand::Explain(a) => {
            let conn = crate::ops::open_readonly(
                &crate::ops::tenant_path(&a.tenant)?,
                &shared_db_path()?,
            )?;
            crate::ops::explain(&conn, &a.sql, &mut std::io::stdout())
        }
    }
}

/// `<db>` → `<db>.bak` beside it.
fn backup_path(db: &Path) -> PathBuf {
    let mut p: OsString = db.as_os_str().to_owned();
    p.push(".bak");
    PathBuf::from(p)
}

fn snapshot() -> anyhow::Result<()> {
    let user = crate::collection::user_db()?;
    let user_bak = backup_path(&user);
    println!("Snapshot {} -> {}", user.display(), user_bak.display());
    snapshot_db(&user, &user_bak)?;

    // The shared catalog is only present once `dumpstar setup` has run; tests
    // that touch shared tables (e.g. price seeding) rely on it being snapshotted.
    let shared = shared_db_path()?;
    if shared.exists() {
        let shared_bak = backup_path(&shared);
        println!("Snapshot {} -> {}", shared.display(), shared_bak.display());
        snapshot_db(&shared, &shared_bak)?;
    }
    Ok(())
}

fn restore() -> anyhow::Result<()> {
    let user = crate::collection::user_db()?;
    let user_bak = backup_path(&user);
    if !user_bak.exists() {
        anyhow::bail!(
            "no snapshot at {} — run `dumpstar db snapshot` first",
            user_bak.display()
        );
    }
    println!("Restore {} <- {}", user.display(), user_bak.display());
    restore_db(&user_bak, &user)?;

    let shared = shared_db_path()?;
    let shared_bak = backup_path(&shared);
    if shared_bak.exists() {
        println!("Restore {} <- {}", shared.display(), shared_bak.display());
        restore_db(&shared_bak, &shared)?;
    }
    Ok(())
}
