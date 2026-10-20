//! Read-only production inspection: `dumpstar db query|explain` and
//! `dumpstar search`.
//!
//! These exist so "what is the data, and why is this slow" is answered in the
//! running container, not on a copy of its files. Read-only is a property of
//! the handle, not of care: the tenant file is opened `SQLITE_OPEN_READ_ONLY`,
//! the catalog is attached through the same read-only attach the server uses
//! (`dumpstar_db::attach_shared_readonly`, so the same TEMP VIEWs), and
//! `PRAGMA query_only` is set on top, so a write statement is refused by
//! SQLite whichever way it is spelled.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, types::ValueRef};

use dumpstar_core::query::parse;
use dumpstar_db::search::{self, Slice};

/// Which tenant: `current` (what `$DUMPSTAR_USER` resolves to, as `serve`
/// does), a handle, or a `database_id`.
pub fn tenant_path(tenant: &str) -> anyhow::Result<PathBuf> {
    if tenant == "current" {
        return crate::collection::user_db();
    }
    if let Some(t) = dumpstar_db::tenants::lookup(tenant)? {
        return Ok(t.path);
    }
    if dumpstar_db::validate_database_id(tenant).is_ok() {
        let path = dumpstar_db::tenant_db_path_for_id(tenant)?;
        if path.exists() {
            return Ok(path);
        }
    }
    anyhow::bail!(
        "no tenant {tenant:?}: not `current`, a live handle, or a database id on this box"
    )
}

/// A read-only tenant connection with the catalog attached, as the server has it.
pub fn open_readonly(user: &Path, shared: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        user,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {} read-only", user.display()))?;
    conn.busy_timeout(dumpstar_db::BUSY_TIMEOUT)?;
    dumpstar_db::attach_shared_readonly(&conn, shared)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

fn open_for(tenant: &str) -> anyhow::Result<Connection> {
    open_readonly(&tenant_path(tenant)?, &dumpstar_db::shared_db_path()?)
}

fn cell(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
    }
}

/// Run `sql`, writing a header and tab-separated rows to `out`; returns the row count.
pub fn query(conn: &Connection, sql: &str, out: &mut impl std::io::Write) -> anyhow::Result<usize> {
    let mut stmt = conn.prepare(sql)?;
    if !stmt.readonly() {
        anyhow::bail!("refusing a statement that writes: this tool is read-only");
    }
    let n = stmt.column_count();
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    writeln!(out, "{}", names.join("\t"))?;
    let mut rows = stmt.query([])?;
    let mut count = 0;
    while let Some(r) = rows.next()? {
        let cells: Vec<String> = (0..n)
            .map(|i| r.get_ref(i).map(cell))
            .collect::<rusqlite::Result<_>>()?;
        writeln!(out, "{}", cells.join("\t"))?;
        count += 1;
    }
    Ok(count)
}

/// `EXPLAIN QUERY PLAN` for `sql`, then the statement run to completion and timed.
pub fn explain(conn: &Connection, sql: &str, out: &mut impl std::io::Write) -> anyhow::Result<()> {
    {
        let stmt = conn.prepare(sql)?;
        if !stmt.readonly() {
            anyhow::bail!("refusing a statement that writes: this tool is read-only");
        }
    }
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        writeln!(out, "{}", r.get::<_, String>(3)?)?;
    }
    let mut stmt = conn.prepare(sql)?;
    let start = Instant::now();
    let mut rows = stmt.query([])?;
    let mut count = 0usize;
    while rows.next()?.is_some() {
        count += 1;
    }
    writeln!(out, "-- {count} rows, wall {:.1} ms", ms(start.elapsed()))?;
    Ok(())
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Arguments for `dumpstar search`.
#[derive(clap::Args)]
pub struct SearchArgs {
    /// `current`, a handle, or a database id.
    #[arg(long, default_value = "current")]
    tenant: String,
    /// The collection search query; empty means the default view.
    query: Option<String>,
    /// A key from the server's sort list (name, number, set, price, …).
    #[arg(long)]
    sort: Option<String>,
    /// `asc` or `desc`.
    #[arg(long)]
    dir: Option<String>,
    /// Widen to the whole catalog, as the "All cards" toggle does.
    #[arg(long)]
    include_unowned: bool,
    /// Rows to print (the server's default page is 250); `all` for everything.
    #[arg(long, default_value = "250")]
    limit: String,
    #[arg(long, default_value_t = 0)]
    offset: u32,
    /// Print per-statement timings after the rows.
    #[arg(long)]
    timing: bool,
}

/// Execute `dumpstar search`.
pub fn run_search(args: SearchArgs) -> anyhow::Result<()> {
    let conn = open_for(&args.tenant)?;
    search_to(&conn, &args, &mut std::io::stdout())
}

fn search_to(
    conn: &Connection,
    args: &SearchArgs,
    out: &mut impl std::io::Write,
) -> anyhow::Result<()> {
    let total_start = Instant::now();
    if let Some(s) = args.sort.as_deref()
        && !search::is_sortable(s)
    {
        anyhow::bail!("sort must be one of: {}", search::SORT_KEYS.join(", "));
    }
    let slice = if args.limit == "all" {
        anyhow::ensure!(args.offset == 0, "offset has no meaning with limit=all");
        Slice::All
    } else {
        Slice::Page {
            limit: args
                .limit
                .parse()
                .context("limit must be `all` or a whole number")?,
            offset: args.offset,
        }
    };

    let t = Instant::now();
    let registry = dumpstar_db::search_meta::load_registry(conn)?;
    let flags = dumpstar_db::search_meta::load_flags(conn)?;
    let load = t.elapsed();

    let t = Instant::now();
    let q = args.query.as_deref().unwrap_or("").trim();
    let mut compiled = if q.is_empty() {
        search::compile_all()
    } else {
        let ast = parse(q, &registry)
            .map_err(|e| anyhow::anyhow!("{} (at {})", e.message, e.position))?;
        search::compile(&ast, &flags)
    };
    if args.include_unowned {
        compiled.set_catalog_wide(true);
    }
    compiled.override_order(args.sort.as_deref(), args.dir.as_deref());
    let compile = t.elapsed();

    let (page, timings) = search::search_page_timed(conn, &compiled, slice)?;
    for r in &page.rows {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.set_code,
            r.number,
            r.name,
            r.variant,
            r.finish,
            r.owned_count,
            r.card_type.as_deref().unwrap_or(""),
            r.market_price
                .map(|p| format!("{p:.2}"))
                .unwrap_or_default(),
        )?;
    }
    writeln!(
        out,
        "-- {} rows shown of {} total, value {}",
        page.rows.len(),
        page.total,
        page.total_value
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "n/a".into()),
    )?;
    if args.timing {
        writeln!(
            out,
            "-- timing ms: registry-load {:.1}, parse+compile {:.1}, totals {}, rows {:.1}, copies {:.1}, total {:.1}",
            ms(load),
            ms(compile),
            timings
                .totals_ms
                .map_or_else(|| "-".to_string(), |d| format!("{d:.1}")),
            timings.rows_ms,
            timings.copies_ms,
            ms(total_start.elapsed()),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::tests::seed_into(dir.path());
        let conn = open_readonly(
            &dir.path().join("collection.sqlite"),
            &dir.path().join("shared.sqlite"),
        )
        .unwrap();
        (dir, conn)
    }

    #[test]
    fn query_reads_tenant_and_attached_catalog_and_refuses_writes() {
        let (_d, conn) = fixture();
        let mut out = Vec::new();
        let n = query(&conn, "SELECT COUNT(*) AS n FROM cards", &mut out).unwrap();
        assert_eq!(n, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("n\n"), "{text}");
        assert_ne!(text.trim_end().lines().last(), Some("0"));

        let err = query(&conn, "DELETE FROM cards", &mut Vec::new()).unwrap_err();
        let _ = err;
        // A write that prepares as read-only is still stopped by the handle.
        assert!(conn.execute_batch("CREATE TABLE x (a)").is_err());
    }

    #[test]
    fn explain_prints_a_plan_and_wall_time() {
        let (_d, conn) = fixture();
        let mut out = Vec::new();
        explain(
            &conn,
            "SELECT * FROM printings WHERE set_code = 'JTL'",
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("SCAN") || text.contains("SEARCH"), "{text}");
        assert!(text.contains("wall"), "{text}");
    }

    #[test]
    fn search_prints_rows_and_timings_through_the_server_path() {
        let (_d, conn) = fixture();
        let args = SearchArgs {
            tenant: "current".into(),
            query: Some("is:owned".into()),
            sort: Some("set".into()),
            dir: None,
            include_unowned: false,
            limit: "250".into(),
            offset: 0,
            timing: true,
        };
        let mut out = Vec::new();
        search_to(&conn, &args, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("-- timing ms:"), "{text}");
        assert!(text.lines().count() > 2, "expected rows: {text}");

        let bad = SearchArgs {
            sort: Some("nope".into()),
            ..args
        };
        assert!(search_to(&conn, &bad, &mut Vec::new()).is_err());
    }
}
