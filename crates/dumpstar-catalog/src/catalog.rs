use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::Result;

const SCHEMA: &str = include_str!("schema.sql");

/// An open `shared.sqlite`. The constructor chosen is the capability: only
/// [`Catalog::open_writer`] yields a connection that can write.
pub struct Catalog {
    pub(crate) conn: Connection,
}

impl Catalog {
    /// Open for the apply job: creates the file and re-applies the schema.
    pub fn open_writer(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Open for serving: SQLite's read-only flag, so any write errors.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}
