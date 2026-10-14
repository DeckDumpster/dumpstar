use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::Result;

/// The SWU catalog schema: the one card model of `shared.sqlite`. dumpstar-db
/// layers its own tables on top of it and applies both.
pub const SCHEMA: &str = include_str!("schema.sql");

/// An open `shared.sqlite`. The constructor chosen is the capability: only
/// [`Catalog::open_writer`] yields a connection that can write.
pub struct Catalog {
    pub(crate) conn: Connection,
    /// The file held another card model (see [`reset_if_legacy`]) and was
    /// emptied before the schema was applied.
    pub rebuilt_legacy: bool,
}

impl Catalog {
    /// Open for the apply job: creates the file and re-applies the schema.
    pub fn open_writer(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let rebuilt_legacy = reset_if_legacy(&conn)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            rebuilt_legacy,
        })
    }

    /// Open for serving: SQLite's read-only flag, so any write errors.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self {
            conn,
            rebuilt_legacy: false,
        })
    }

    /// A resume point recorded by a finished fetch (see [`crate::swu`]).
    pub fn sync_state(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row("SELECT value FROM sync_state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Columns that exist only in an earlier card model (a card belongs to one
/// set and carries a supertype; a printing is priced by a TCGplayer
/// product). Any one of them means the file is not
/// ours.
const LEGACY_COLUMNS: &[(&str, &str)] = &[
    ("cards", "supertype"),
    ("cards", "set_code"),
    ("cards", "number_sortable"),
    ("sets", "series"),
    ("sets", "ptcgo_code"),
    ("printings", "tcgplayer_product_id"),
];

/// Whether `conn` holds a catalog from the legacy card model.
pub fn is_legacy(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?;
    for (table, column) in LEGACY_COLUMNS {
        if stmt.exists([table, column])? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Empty a catalog written by the legacy card model, so the SWU schema can be
/// applied to it. `shared.sqlite` is derived data, rebuilt from the
/// upstreams; the tenant files are other files and are not touched. Returns
/// whether anything was dropped.
///
/// Every table, view, trigger and virtual table goes, including the ones
/// dumpstar-db layers on top (they are re-created by its next convergence).
/// A `CREATE TABLE IF NOT EXISTS` against the legacy `sets` would otherwise be
/// skipped and the next statement would fail on a column it does not have.
pub fn reset_if_legacy(conn: &Connection) -> Result<bool> {
    if !is_legacy(conn)? {
        return Ok(false);
    }
    // (kind, name), kind being the DROP keyword: a virtual table is listed as
    // 'table' by sqlite_master and is told apart by its SQL.
    let objects: Vec<(&'static str, String)> = {
        let mut stmt = conn.prepare(
            "SELECT type, name, sql FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' AND type IN ('trigger', 'view', 'table')",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind: String = r.get(0)?;
            let name: String = r.get(1)?;
            let sql: Option<String> = r.get(2)?;
            Ok((kind, name, sql.unwrap_or_default()))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, name, sql) = row?;
            let kind = match kind.as_str() {
                "trigger" => "TRIGGER",
                "view" => "VIEW",
                _ if sql
                    .trim_start()
                    .to_ascii_uppercase()
                    .starts_with("CREATE VIRTUAL") =>
                {
                    "VTABLE"
                }
                _ => "TABLE",
            };
            out.push((kind, name));
        }
        out
    };
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let dropped = (|| -> rusqlite::Result<()> {
        // Triggers and views first, then virtual tables (which take their
        // shadow tables with them), then the rest; `IF EXISTS` covers a shadow
        // table already gone.
        for kind in ["TRIGGER", "VIEW", "VTABLE", "TABLE"] {
            for (_, name) in objects.iter().filter(|(k, _)| *k == kind) {
                let keyword = if kind == "VTABLE" { "TABLE" } else { kind };
                conn.execute_batch(&format!(
                    "DROP {keyword} IF EXISTS \"{}\"",
                    name.replace('"', "\"\"")
                ))?;
            }
        }
        Ok(())
    })();
    match dropped {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.into());
        }
    }
    Ok(true)
}

/// The objects [`LEGACY_SHAPE_DDL`] creates that no part of the current schema
/// has, so a test can assert a rebuild dropped them.
pub const LEGACY_ONLY_OBJECTS: &[&str] = &[
    "latest_prices",
    "set_aliases",
    "legacy_t",
    "legacy_fts",
    "latest_sealed_prices",
];

/// A catalog as the earlier card model wrote it, cut down to what collides
/// with this schema: tables, a view, a trigger, a virtual table, a few rows,
/// `user_version = 2`. For tests of [`reset_if_legacy`] here and in the crates
/// that call it; this file is the only place in the repository that names the
/// legacy columns.
pub const LEGACY_SHAPE_DDL: &str = "
CREATE TABLE sets (set_code TEXT PRIMARY KEY, ptcgo_code TEXT, name TEXT NOT NULL, series TEXT NOT NULL);
CREATE TABLE cards (card_id TEXT PRIMARY KEY, set_code TEXT NOT NULL REFERENCES sets(set_code),
                    number TEXT NOT NULL, number_sortable INTEGER NOT NULL, name TEXT NOT NULL,
                    supertype TEXT, attacks TEXT);
CREATE TABLE printings (printing_id TEXT PRIMARY KEY, card_id TEXT NOT NULL REFERENCES cards(card_id),
                        variant TEXT NOT NULL, tcgplayer_product_id INTEGER);
CREATE TABLE prices (id INTEGER PRIMARY KEY, tcgplayer_product_id INTEGER, price REAL);
CREATE TABLE latest_prices (tcgplayer_product_id INTEGER, price REAL);
CREATE TABLE set_aliases (alias TEXT PRIMARY KEY, set_code TEXT NOT NULL);
CREATE VIEW latest_sealed_prices AS SELECT * FROM prices;
CREATE TRIGGER legacy_t AFTER INSERT ON prices BEGIN SELECT 1; END;
CREATE VIRTUAL TABLE legacy_fts USING fts5(name);
INSERT INTO sets VALUES ('old', 'OLD', 'Test Set', 'Test Series');
INSERT INTO cards VALUES ('old-1', 'old', '1', 1, 'Legacy Card', 'Legacy', '[]');
INSERT INTO printings VALUES ('old-1-normal', 'old-1', 'normal', 1);
PRAGMA user_version = 2;
";
