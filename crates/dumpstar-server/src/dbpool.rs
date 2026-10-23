//! Reads that never queue behind another request, and a bound on every database call.
//!
//! A tenant has one writer connection behind a mutex (the kit's `Tenants::connection`). Every
//! request used to take it, so one slow call held every other request for that tenant. A read now
//! checks out its own read-only connection from [`checkout`] — the same file, opened through the
//! kit's `connect_tenant` so it is asserted isolated — and WAL lets any number of them run while a
//! write is in flight. Writes still take the writer mutex and stay serialised.
//!
//! [`bounded`] arms a watchdog on whichever connection a call holds: past [`DB_BOUND`] it
//! interrupts the running statement, so the call fails with a named error instead of holding the
//! connection.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dumpster_kit_tenancy::TenantId;
use rusqlite::Connection;

use dumpstar_db::DbError;

/// The longest any one request may hold a database connection.
pub const DB_BOUND: Duration = Duration::from_secs(30);

/// Idle read connections kept per tenant; more than this many concurrent reads open extras that
/// are dropped when they finish.
const IDLE_PER_TENANT: usize = 8;

type Idle = Mutex<HashMap<PathBuf, Vec<Connection>>>;

fn idle() -> &'static Idle {
    static IDLE: OnceLock<Idle> = OnceLock::new();
    IDLE.get_or_init(Default::default)
}

/// A pooled read-only connection; returns to the pool when dropped.
pub struct Reader {
    conn: Option<Connection>,
    key: PathBuf,
}

impl Deref for Reader {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("reader holds a connection")
    }
}

impl DerefMut for Reader {
    fn deref_mut(&mut self) -> &mut Connection {
        self.conn.as_mut().expect("reader holds a connection")
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut map = idle().lock().unwrap_or_else(|e| e.into_inner());
            let v = map.entry(self.key.clone()).or_default();
            if v.len() < IDLE_PER_TENANT {
                v.push(conn);
            }
        }
    }
}

fn paths() -> &'static Mutex<HashMap<String, (PathBuf, PathBuf)>> {
    static PATHS: OnceLock<Mutex<HashMap<String, (PathBuf, PathBuf)>>> = OnceLock::new();
    PATHS.get_or_init(Default::default)
}

/// A read-only connection for tenant `id`, whose writer connection is `writer`. The writer is
/// locked only to learn the tenant's two file paths, once per process; after that a read never
/// touches it, so a write in flight cannot make a read wait.
pub fn checkout(id: &TenantId, writer: &Mutex<Connection>) -> Result<Reader, DbError> {
    let known = paths()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id.as_str())
        .cloned();
    let (user, shared) = match known {
        Some(p) => p,
        None => {
            let w = writer.lock().unwrap_or_else(|e| e.into_inner());
            let user = w
                .path()
                .map(PathBuf::from)
                .ok_or_else(|| DbError::Env("tenant connection has no file".into()))?;
            let shared: String = w.query_row(
                "SELECT file FROM pragma_database_list WHERE name = 'shared'",
                [],
                |r| r.get(0),
            )?;
            let p = (user, PathBuf::from(shared));
            paths()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.as_str().to_string(), p.clone());
            p
        }
    };
    let pooled = idle()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(&user)
        .and_then(Vec::pop);
    let conn = match pooled {
        Some(c) => c,
        None => {
            let c = dumpster_kit_tenancy::connection::connect_tenant(
                &user,
                &shared,
                &dumpstar_db::user_spec(),
            )
            .map_err(|e| DbError::Env(e.to_string()))?;
            c.execute_batch("PRAGMA query_only = ON")?;
            c
        }
    };
    Ok(Reader {
        conn: Some(conn),
        key: user,
    })
}

/// Runs `f` on `conn` under the [`DB_BOUND`] watchdog. A statement still running at the bound is
/// interrupted; the error then names `what` and the bound.
pub fn bounded<T, E>(
    conn: &mut Connection,
    what: &str,
    f: impl FnOnce(&mut Connection) -> Result<T, E>,
    on_timeout: impl FnOnce(String) -> E,
) -> Result<T, E> {
    bounded_for(conn, what, DB_BOUND, f, on_timeout)
}

fn bounded_for<T, E>(
    conn: &mut Connection,
    what: &str,
    bound: Duration,
    f: impl FnOnce(&mut Connection) -> Result<T, E>,
    on_timeout: impl FnOnce(String) -> E,
) -> Result<T, E> {
    let fired = Arc::new(AtomicBool::new(false));
    let done = Arc::new(Mutex::new(false));
    let (stop, wait) = mpsc::channel::<()>();
    let handle = conn.get_interrupt_handle();
    let watchdog = {
        let (fired, done) = (fired.clone(), done.clone());
        std::thread::spawn(move || {
            if wait.recv_timeout(bound) == Err(mpsc::RecvTimeoutError::Timeout) {
                let done = done.lock().unwrap_or_else(|e| e.into_inner());
                if !*done {
                    fired.store(true, Ordering::SeqCst);
                    handle.interrupt();
                }
            }
        })
    };
    let result = f(conn);
    *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
    drop(stop);
    let _ = watchdog.join();
    match result {
        Err(_) if fired.load(Ordering::SeqCst) => Err(on_timeout(format!(
            "{what} exceeded the {} s database bound and was stopped",
            bound.as_secs_f64()
        ))),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statement_past_the_bound_is_stopped_and_named() {
        let mut conn = Connection::open_in_memory().unwrap();
        let started = std::time::Instant::now();
        let r: Result<i64, String> = bounded_for(
            &mut conn,
            "search_cards",
            Duration::from_millis(200),
            |c| {
                c.query_row(
                    "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 2000000000) \
                     SELECT count(*) FROM n",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            },
            |m| m,
        );
        let err = r.unwrap_err();
        assert!(
            err.contains("search_cards") && err.contains("0.2 s"),
            "{err}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_call_inside_the_bound_is_untouched() {
        let mut conn = Connection::open_in_memory().unwrap();
        let r: Result<i64, String> = bounded_for(
            &mut conn,
            "get_card",
            Duration::from_secs(5),
            |c| {
                c.query_row("SELECT 7", [], |r| r.get(0))
                    .map_err(|e| e.to_string())
            },
            |m| m,
        );
        assert_eq!(r.unwrap(), 7);
    }
}
