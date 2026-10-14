//! Opening and wiring up Dumpstar's databases.
//!
//! The shared catalog is opened read-write only by `dumpstar setup` and the
//! ingest pipelines. A per-user collection database `ATTACH`es the catalog
//! read-only and exposes its tables through `TEMP VIEW`s so queries can join
//! user and catalog data unqualified (PLAN.md §3.1).
//!
//! Schema management: the full schema lives in `schema_shared.sql` /
//! `schema_user.sql` and is re-applied with `CREATE … IF NOT EXISTS` on
//! every open. No migration history, no refinery — additive change travels
//! by idempotent re-application (dumpstar-luo).
//!
//! What that cannot express — a change that transforms or drops — is gated
//! instead of applied: every database carries its schema version in
//! `PRAGMA user_version`, and a file written by a newer build is REFUSED
//! rather than opened. See [`crate::schema_version`] for the three
//! outcomes and why the refusal is what makes rollback safe (pd-ja38).

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use rusqlite::backup::Backup;

use crate::error::Result;
use crate::schema_version::{self};
use dumpster_kit_tenancy::{TenancyError, TenantDbSpec};

/// How long an open in this crate waits for a lock before giving up. Every
/// `open_*` sets it, and [`checkpoint_truncate`] restores it after borrowing
/// the connection's busy handler for its own, shorter wait.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The catalog's own schema (the one card model) and what this crate layers
/// on it, applied in that order.
pub(crate) const SCHEMA_OVERLAY: &str = include_str!("schema_shared.sql");
const SCHEMA_USER: &str = include_str!("schema_user.sql");

/// How long a catalog open waits for another writer before giving up.
///
/// Five seconds is the ordinary answer, and it is the right one for a
/// command an operator is watching: `dumpstar setup` and `dumpstar data
/// expand-only` want to say "something else is writing this" quickly rather
/// than sit there.
const OPEN_PATIENCE: Duration = Duration::from_secs(5);

/// The patience `dumpstar serve` uses on the rare start that really does have
/// to converge the catalog — see [`open_shared_for_serving`].
///
/// **Derived, not chosen.** The only other writer on a deployed box is the
/// nightly `dumpstar-lake-derive shared`, whose own unit allows it
/// `TimeoutStartSec=1800`; waiting that long would be the honest number and
/// is useless, because `deploy/dumpstar.container` gives the server
/// `TimeoutStartSec=120` and systemd kills the start job first. So the wait
/// is bounded by what the server's own unit permits, with headroom for the
/// rest of startup — a start that is going to fail should fail while systemd
/// is still listening, and say why.
///
/// `tests/deploy/run.sh` holds this against `dumpstar.container`, because the
/// two numbers live in files that cannot share a constant and a unit whose
/// timeout drops below this one would turn a diagnosable refusal back into a
/// `SIGTERM` with nothing in the journal.
const SERVING_PATIENCE: Duration = Duration::from_secs(90);

/// Open the shared catalog database, creating it if absent, and apply the
/// schema (idempotent — every CREATE is IF NOT EXISTS). Read-write — for
/// `dumpstar setup` and ingest only.
///
/// PRAGMAs tuned for the variant-expansion write workload, which opens
/// ~20k per-card transactions: WAL keeps writes sequential, synchronous
/// = NORMAL drops the per-commit fsync (still crash-safe in WAL mode),
/// and a 64MB page cache keeps the printings + indices hot through the
/// full expansion pass. Without these, throughput collapses ~3× once
/// the table exceeds the default ~2MB cache (dumpstar-rqr).
///
/// After schema init, backfills the sealed products and reconciles the search
/// query metadata seed files, so a freshly-opened DB is always ready. Cheap and idempotent on the
/// existing prod DB. See dumpstar-luo.
///
/// Cheap is not free, though, and it is never a no-op:
/// [`crate::search_meta::reconcile`] is a `DELETE` and a few hundred
/// `INSERT`s every time. **Anything that only needs the catalog to BE
/// converged, rather than to converge it, wants
/// [`open_shared_for_serving`]** — see pd-dzu5 and
/// [`crate::convergence`].
pub fn open_shared(path: &Path) -> Result<Connection> {
    open_shared_with_patience(path, OPEN_PATIENCE)
}

/// [`open_shared`], waiting `patience` for another writer rather than the
/// default five seconds.
///
/// The patience is the caller's because the callers are not alike: a
/// hand-run command wants to fail fast and be told, and a server start that
/// has landed inside the nightly catalog build wants to wait it out. It
/// changes nothing else — same PRAGMAs, same convergence, same stamp.
pub fn open_shared_with_patience(path: &Path, patience: Duration) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(patience)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; \
         PRAGMA synchronous = NORMAL; \
         PRAGMA cache_size = -65536; \
         PRAGMA foreign_keys = ON;",
    )?;
    // Before a single statement of schema runs: a catalog written by a newer
    // build is refused, not migrated backwards into.
    schema_version::gate(&conn, schema_version::Shared)?;
    // A catalog written by the Pokémon-shaped schema this project was forked
    // with is emptied, not migrated: it is derived data (sp-r7w5xi).
    if dumpstar_catalog::reset_if_legacy(&conn).map_err(|e| match e {
        dumpstar_catalog::CatalogError::Sqlite(e) => crate::DbError::Sqlite(e),
        other => crate::DbError::Conflict(other.to_string()),
    })? {
        println!(
            "dumpstar: {} held another card model; emptied it for the Star Wars: Unlimited \
             catalog (rebuilt by `dumpstar catalog-apply`)",
            path.display()
        );
    }
    converge(&mut conn)?;
    Ok(conn)
}

/// Everything this build writes into a catalog it opens read-write, in the
/// one place that does it.
///
/// Kept as its own function because [`crate::convergence`] hashes exactly its
/// inputs: a seed reconciled here and not named there is the one way the
/// fingerprint can lie, and the two being adjacent is what makes the drift
/// gate a thing somebody is likely to run into.
fn converge(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(dumpstar_catalog::SCHEMA)?;
    conn.execute_batch(SCHEMA_OVERLAY)?;
    backfill_sealed(conn)?;
    // The search query language's metadata (keywords, rarity ranks,
    // `is:`-flag definitions). It used to be reconciled by each caller
    // instead — `dumpstar setup`, the derive, `dumpstar serve`, the fixture
    // seeder — which made it the one shipped seed that was not part of
    // opening the catalog, and therefore the one nothing could account for
    // when asking whether a catalog was converged (pd-dzu5).
    crate::search_meta::reconcile(conn)?;
    // Stamped last but one: the file claims this shape only once it has it.
    schema_version::stamp(conn, schema_version::Shared)?;
    // And last of all, the claim that all of the above has happened. After
    // the stamp, deliberately: a fingerprint recorded before the work would
    // let a convergence that died halfway be skipped forever.
    crate::convergence::record(conn)?;
    Ok(())
}

/// Open the shared catalog for a process that must *have* it converged but is
/// not the thing that builds it — i.e. `dumpstar serve`.
///
/// Returns a read-only connection when the catalog already carries this
/// build's convergence fingerprint, having taken **no write lock at all**;
/// otherwise converges it read-write, exactly as [`open_shared`] would, and
/// hands that connection back.
///
/// This is the whole of pd-dzu5's fix. The server's startup convergence is
/// deliberate — a binary upgrade can ship a data-only migration that must be
/// applied before the first request — but it ran unconditionally, so *every*
/// restart competed for the catalog's write lock with the nightly
/// `dumpstar-lake-derive shared`, which holds it for minutes. Losing that race
/// is a start that fails on `database is locked` after five seconds and a
/// `Restart=on-failure` loop for the rest of the build, with no `OnFailure=`
/// on `dumpstar.container` to say so. Now an ordinary restart — same binary,
/// same seeds — asks one read-only question and is done.
///
/// **Every answer that is not a clear yes is a no**, and the fall-through is
/// to do the work: a catalog that does not exist yet, one from a build older
/// than the fingerprint table, one whose row is missing, and a WAL database
/// whose `-shm` is absent (which cannot be opened read-only at all) each land
/// on [`open_shared_with_patience`]. Note which case that last one is: the
/// `-shm` is present exactly while another process holds the catalog open, so
/// the read-only probe works precisely on the night it matters and the
/// fall-through is taken when there is no competing writer to lose to.
///
/// One of those swallowed outcomes deserves saying out loud: a catalog written
/// by a NEWER build is refused by [`schema_version::gate`], and the probe
/// discards that refusal like any other failure. It is not lost — the
/// read-write open gates again, on the same file, and refuses with the same
/// message. The probe is allowed exactly one verdict, "already converged", and
/// nothing else it learns is acted on.
pub fn open_shared_for_serving(path: &Path) -> Result<Connection> {
    if let Ok(conn) = open_shared_readonly(path)
        && crate::convergence::is_converged(&conn)
    {
        return Ok(conn);
    }
    println!(
        "dumpstar: the catalog at {} is not converged to this build — applying the schema and \
         shipped seeds before serving. This is the only start that writes the catalog, and the \
         only one that can be delayed by a `dumpstar-lake-derive shared` already holding it.",
        path.display()
    );
    open_shared_with_patience(path, SERVING_PATIENCE)
}

/// Fold the catalog's sealed products (tcgcsv products with no collector
/// number) into `sealed_products`, where the application reads them. Insert if
/// absent: a name or group the catalog later changes is not chased, and a
/// product a tenant already holds never changes id.
fn backfill_sealed(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO sealed_products (product_id, set_code, name, category, fetched_at) \
         SELECT t.product_id, \
                (SELECT s.set_code FROM sets s WHERE s.set_code = g.abbreviation), \
                t.name, 'sealed', '' \
           FROM tcgcsv_products t \
           JOIN tcgcsv_groups g ON g.group_id = t.group_id \
          WHERE t.number IS NULL",
    )?;
    Ok(())
}

/// Open the shared catalog **read-only**.
///
/// No schema application, no seed reconciliation, no version stamp — and no
/// writes, enforced by SQLite rather than by review. `SQLITE_OPEN_READ_ONLY`
/// makes an attempted write an error at the connection, so "this caller does
/// not write the catalog" is a property of the handle rather than a claim
/// about the code that holds it.
///
/// That is the whole reason it exists (pd-lunn). Since the derivation left
/// `dumpstar data refresh`, the refresh's only interest in the catalog is one
/// question — which sets it already has, so it knows which cards to fetch —
/// and the acceptance criterion for that change is that the command writes no
/// catalog table at all. A read-only handle cannot, including from code
/// nobody has written yet.
///
/// The file must already exist: creating it is `dumpstar setup`'s job, and a
/// refresh that quietly built an empty catalog would land every set's cards
/// every night rather than the handful that are new.
pub fn open_shared_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // The same refusal `open_shared` makes, in the same place: a catalog
    // written by a newer build is not one this build may read rows out of and
    // act on. Reading is what this handle is for, so the gate is the whole of
    // the check — there is nothing here to stamp.
    schema_version::gate(&conn, schema_version::Shared)?;
    Ok(conn)
}

/// The same convergence for `schema_user.sql`. A collection created between
/// pd-5m54 and pd-385w already carries `ownership_outbox`, so the amended
/// `CREATE TABLE IF NOT EXISTS` above does nothing to it and the provenance
/// column arrives only here. Same for `zone_holdings_run.sealed_rows` on any
/// collection that has been read back from the zone at least once.
///
/// This one is not defaultless, and that is the point rather than an
/// exception to the rule above: every event such a collection already holds
/// was written by a trigger, so `DEFAULT 'trigger'` states what is true of
/// all of them. There is no backfill to do — which is what makes it
/// expressible as an `ALTER` at all.
///
/// **No `user_version` bump.** The gate exists to stop an older binary that
/// would get a collection *wrong*; one that has never heard of `source`
/// reads the outbox exactly as it did before and writes events a newer
/// build labels correctly by default. Refusing to open would be a rollback
/// broken for a column that costs nothing to ignore — the same reasoning
/// `schema_user.sql` records for dropping `refinery_schema_history`.
const USER_ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "ownership_outbox",
        "source",
        "ALTER TABLE ownership_outbox ADD COLUMN source TEXT NOT NULL \
         DEFAULT 'trigger' CHECK (source IN ('trigger', 'backfill', 'redrive'))",
    ),
    // pd-bbv7. A collection read back from the zone before sealed had a
    // staging table has a `zone_holdings_run` row that counts only its
    // singles, and `0` is the true number of sealed lots that read
    // materialised — so the default is a statement about those rows rather
    // than a placeholder in them. No `user_version` bump, for the reason
    // above it: an older binary writes the row without this column and reads
    // the collection exactly as it did before.
    (
        "zone_holdings_run",
        "sealed_rows",
        "ALTER TABLE zone_holdings_run ADD COLUMN sealed_rows INTEGER NOT NULL DEFAULT 0",
    ),
    // sp-eocx2y. A want gains a minimum condition and a quantity. Existing
    // wants were each for one copy of any condition, which is what the
    // defaults say; no `user_version` bump, an older binary ignores them.
    (
        "wishlist",
        "min_condition",
        "ALTER TABLE wishlist ADD COLUMN min_condition TEXT",
    ),
    (
        "wishlist",
        "quantity",
        "ALTER TABLE wishlist ADD COLUMN quantity INTEGER NOT NULL DEFAULT 1 CHECK (quantity >= 1)",
    ),
    (
        "wishlist",
        "quantity_have",
        "ALTER TABLE wishlist ADD COLUMN quantity_have INTEGER NOT NULL DEFAULT 0",
    ),
];

fn add_missing_user_columns(conn: &Connection) -> Result<()> {
    add_columns(conn, USER_ADDED_COLUMNS)
}

fn add_columns(conn: &Connection, columns: &[(&str, &str, &str)]) -> Result<()> {
    for (table, column, ddl) in columns {
        let present: bool = conn
            .prepare(&format!(
                "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"
            ))?
            .exists([column])?;
        if !present {
            conn.execute_batch(ddl)?;
        }
    }
    Ok(())
}

/// `ATTACH` the shared catalog read-only as `shared` with its `TEMP VIEW`s, version-gated
/// (the kit's [`dumpster_kit_tenancy::attach_shared_readonly`], with Dumpstar's catalog version).
pub fn attach_shared_readonly(conn: &Connection, shared_path: &Path) -> Result<()> {
    Ok(dumpster_kit_tenancy::attach_shared_readonly(
        conn,
        shared_path,
        crate::app::SHARED,
    )?)
}

/// What Dumpstar contributes to a tenant connection: its collection schema, its two schema
/// versions, and the work `CREATE … IF NOT EXISTS` cannot express (added columns, seeded
/// conditions).
pub fn user_spec() -> TenantDbSpec {
    let mut spec = TenantDbSpec::new(SCHEMA_USER, crate::app::USER, crate::app::SHARED);
    spec.prepare = |conn| {
        add_missing_user_columns(conn).map_err(into_tenancy)?;
        crate::conditions::seed_defaults(conn).map_err(into_tenancy)?;
        Ok(())
    };
    spec
}

fn into_tenancy(e: crate::error::DbError) -> TenancyError {
    use crate::error::DbError as D;
    match e {
        D::Sqlite(e) => TenancyError::Sqlite(e),
        D::NotFound(m) => TenancyError::NotFound(m),
        D::Conflict(m) => TenancyError::Conflict(m),
        D::SchemaVersion(m) => TenancyError::SchemaVersion(m),
        other => TenancyError::Env(other.to_string()),
    }
}

/// Open a per-user collection database — applying the user schema — without
/// the shared catalog. For work that touches only user tables (the JSON
/// backup), which must also run on a box where `dumpstar setup` has not built
/// a catalog yet.
///
/// Seeds the collection's `conditions` with the five defaults if they are
/// absent (pd-s4c2). One mechanism covers both cases the move created: a
/// brand-new collection is born with its multipliers, and one written before
/// the table lived here grows them on its next open. Insert-if-absent, so an
/// already-seeded collection is not written to at all.
pub fn open_user(user_path: &Path) -> Result<Connection> {
    Ok(dumpster_kit_tenancy::open_tenant(user_path, &user_spec())?)
}

/// Open the user registry database, creating it if absent, and apply the registry schema
/// (gated and stamped by the kit). Never attaches the catalog.
pub fn open_registry(path: &Path) -> Result<Connection> {
    Ok(dumpster_kit_tenancy::open_registry(path)?)
}

/// Open a per-user collection database — applying the user schema — with the shared catalog
/// attached read-only, refused unless the connection is isolated.
pub fn connect_user(user_path: &Path, shared_path: &Path) -> Result<Connection> {
    Ok(dumpster_kit_tenancy::connect_tenant(
        user_path,
        shared_path,
        &user_spec(),
    )?)
}

/// Apply the user schema to an arbitrary connection. Used by tests that open an in-memory user
/// DB without going through `connect_user`.
pub fn init_user_schema(conn: &Connection) -> Result<()> {
    Ok(dumpster_kit_tenancy::init_tenant_schema(
        conn,
        &user_spec(),
    )?)
}

/// What one truncating WAL checkpoint achieved.
///
/// `reset` is the only field that matters to a caller deciding whether the
/// disk came back. A checkpoint that could not reset still did useful work —
/// it copies frames into the database either way — but the `-wal` file keeps
/// every byte it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpoint {
    /// The WAL was restarted from frame 0 and the file truncated to nothing.
    /// False means a reader was in flight for the whole wait: the frames were
    /// copied out, but the file is the size it was.
    pub reset: bool,
    /// Frames in the WAL when the checkpoint ran. `-1` if the database is not
    /// in WAL mode at all, in which case there was never anything to reclaim.
    pub wal_frames: i64,
    /// Frames copied into the database file.
    pub checkpointed: i64,
}

/// Run `PRAGMA wal_checkpoint(TRUNCATE)` and report what it managed, waiting
/// at most `wait` for readers to get out of the way.
///
/// **This is the only thing that gives the catalog's WAL back** (pd-t50h). In
/// WAL mode a checkpoint can copy frames into the database while readers are
/// active, but it cannot *reset* the WAL — restart it at frame 0 — until a
/// moment arrives with nobody reading. Until then the writer appends, so the
/// file grows for the whole write window and then stays at its high-water mark
/// until something truncates it. Nothing does, on its own: an autocheckpoint
/// runs on a commit, and once the derive exits there is no writer left to
/// commit anything, so the file sits there until the next night.
///
/// A blocked checkpoint is **not an error**. It is a fact about who was
/// reading, it is reported rather than raised, and the caller decides whether
/// it is worth saying out loud. Raising would make a browsing session able to
/// fail the nightly catalog build, which is the wrong trade by a long way.
///
/// `wait` borrows the connection's busy handler and [`BUSY_TIMEOUT`] is
/// restored afterwards, so the value is scoped to the checkpoint rather than
/// to the connection.
pub fn checkpoint_truncate(conn: &Connection, wait: Duration) -> Result<WalCheckpoint> {
    conn.busy_timeout(wait)?;
    let out = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        Ok(WalCheckpoint {
            reset: r.get::<_, i64>(0)? == 0,
            wal_frames: r.get(1)?,
            checkpointed: r.get(2)?,
        })
    });
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(out?)
}

/// How often a long write window stops to try to give the WAL back.
///
/// Five seconds. The cost of an attempt is bounded by [`RECLAIM_WAIT`], so a
/// ten-minute derive pays at most a few seconds in total for it; the benefit
/// is that the WAL is bounded by what one five-second slice of writing
/// produces instead of by the whole window.
const RECLAIM_EVERY: Duration = Duration::from_secs(5);

/// How long an *opportunistic* reclaim waits for readers.
///
/// A tenth of a second, and the shortness is the design. This checkpoint is
/// speculative — there will be another one along in [`RECLAIM_EVERY`] — so
/// blocking on it buys nothing and costs the write window directly. Measured
/// on the mechanism in pd-t50h (a writer committing against one continuously
/// reading connection, ten-second window):
///
/// | reclaim | checkpoint wait | writer commits | peak WAL |
/// |---|---|---|---|
/// | none    | —      | 60,068 | 208.8 MiB |
/// | every 1s | 5s    | 39,740 |  26.8 MiB |
/// | every 1s | 100ms | 61,036 |  42.0 MiB |
///
/// The five-second wait buys a slightly smaller file for a third of the
/// writer's throughput. The short wait keeps the throughput and still takes a
/// factor of five off the file, because a reader that queries in a loop is
/// between transactions often enough for a checkpoint to slip through.
const RECLAIM_WAIT: Duration = Duration::from_millis(100);

/// How long the checkpoint that ENDS a write window waits for readers.
///
/// Thirty seconds, against the hundred milliseconds [`RECLAIM_WAIT`] spends.
/// The trade is the opposite one at this point: the writing is over, so there
/// is no throughput left to protect, and every second spent here is a second
/// that might save the file sitting on the data volume until tomorrow.
const FINAL_RECLAIM_WAIT: Duration = Duration::from_secs(30);

/// Give the catalog's WAL back, at the end of the write window that built it.
///
/// The counterpart to [`WalReclaim`], which bounds the file *during* a long
/// pass; this is what returns it afterwards. Nothing else on the box ever
/// will: an autocheckpoint runs on a commit, and once the process that built
/// the catalog has exited there is no writer left to commit anything, so a
/// `-wal` left at its high-water mark stays there until the next night.
///
/// Called by every **process** that writes `shared.sqlite` and exits, after
/// its own last write: `dumpstar-lake-derive shared` and `dumpstar setup`. A
/// process rather than a function, and the distinction earned itself — put at
/// the end of `dumpstar_derive::derive` it ran before the derive binary wrote
/// `raw_derivation`, and left the provenance row's frames in a file nothing
/// would ever truncate.
///
/// One function rather than the same six lines at each call site, because the
/// wait and the sentence an operator reads are the decision — a second
/// spelling of either is how the two writers start answering the same night
/// differently.
///
/// Blocked is reported and returned, never raised: a reader that held a
/// transaction across this moment has cost some disk until the next run, and
/// failing a catalog build over it would let a browsing session break the
/// nightly job.
pub fn reclaim_catalog_wal(conn: &Connection) -> Result<WalCheckpoint> {
    let out = checkpoint_truncate(conn, FINAL_RECLAIM_WAIT)?;
    if out.reset {
        println!("  catalog WAL reclaimed ({} frames)", out.checkpointed);
    } else {
        eprintln!(
            "!! catalog WAL NOT reclaimed: a reader was in flight for the whole {}s wait, so \
             {} frames were copied out but the -wal file keeps its size until something \
             checkpoints it again. Disk only — the catalog itself is complete.",
            FINAL_RECLAIM_WAIT.as_secs(),
            out.checkpointed,
        );
    }
    Ok(out)
}

/// Drives [`checkpoint_truncate`] from inside a long write loop, at most once
/// every [`RECLAIM_EVERY`].
///
/// Held by the phase doing the writing rather than by the connection, because
/// the period is about *this* write window: a caller that makes one commit and
/// stops has nothing to reclaim and should not be paying for a checkpoint.
///
/// Time-based rather than counted, because the same loop runs at 5,000 cards a
/// second on a warm catalog and at a small fraction of that on a cold one — a
/// count that is right for one is wrong for the other by two orders of
/// magnitude.
pub struct WalReclaim {
    period: Duration,
    last: Instant,
}

impl Default for WalReclaim {
    fn default() -> Self {
        Self::new()
    }
}

impl WalReclaim {
    /// Start the clock at the production period, [`RECLAIM_EVERY`]. The first
    /// reclaim is that far away, so a short loop never checkpoints at all.
    pub fn new() -> Self {
        Self::every(RECLAIM_EVERY)
    }

    /// As [`WalReclaim::new`], with the period named.
    ///
    /// Exists for tests, and it takes the period rather than a "check every
    /// time" flag deliberately: a test that switched the periodicity OFF would
    /// be exercising a code path production never takes. Every caller in the
    /// binary uses [`WalReclaim::new`].
    pub fn every(period: Duration) -> Self {
        Self {
            period,
            last: Instant::now(),
        }
    }

    /// Checkpoint if the period has elapsed, otherwise do nothing.
    ///
    /// `Ok(None)` means "not yet". An error from the checkpoint itself is
    /// returned; a checkpoint a reader blocked is `Ok(Some(..))` with
    /// [`WalCheckpoint::reset`] false, because that is a normal outcome and
    /// not a failure of this run.
    pub fn maybe(&mut self, conn: &Connection) -> Result<Option<WalCheckpoint>> {
        if self.last.elapsed() < self.period {
            return Ok(None);
        }
        let out = checkpoint_truncate(conn, RECLAIM_WAIT)?;
        // Reset from *after* the checkpoint, not from the deadline it passed:
        // a blocked attempt costs RECLAIM_WAIT, and dating the next one from
        // the deadline would make a long stall spend its whole budget again
        // immediately.
        self.last = Instant::now();
        Ok(Some(out))
    }
}

/// Snapshot a live SQLite database to `dest`, overwriting it.
///
/// WAL-correct: uses SQLite's online backup API, which captures a
/// transactionally-consistent view — including any committed WAL frames —
/// even while the server holds the database open. Backs the UI test
/// harness's per-test isolation, replacing the old in-container
/// `python3 sqlite3.backup()` (dumpstar-0g3).
pub fn snapshot_db(live: &Path, dest: &Path) -> Result<()> {
    // A leftover -wal/-shm beside a stale snapshot would shadow the bytes we
    // copy in; start from a clean destination.
    remove_db_files(dest);
    let src = Connection::open(live)?;
    src.busy_timeout(Duration::from_secs(10))?;
    let mut dst = Connection::open(dest)?;
    let backup = Backup::new(&src, &mut dst)?;
    backup.run_to_completion(256, Duration::from_millis(50), None)?;
    Ok(())
}

/// Restore a live SQLite database from a snapshot taken by [`snapshot_db`].
///
/// WAL-correct: copies the snapshot *into* the live database through the
/// online backup API, committing every page as a fresh transaction. This is
/// what makes it safe while the server holds the database open — a plain
/// `cp` of the main file leaves the live `-wal`/`-shm` in place, so the next
/// read replays a prior test's frames on top of the restored bytes and sees
/// the mutated state (dumpstar-lxm).
pub fn restore_db(snapshot: &Path, live: &Path) -> Result<()> {
    let src = Connection::open(snapshot)?;
    let mut dst = Connection::open(live)?;
    dst.busy_timeout(Duration::from_secs(10))?;
    let backup = Backup::new(&src, &mut dst)?;
    backup.run_to_completion(256, Duration::from_millis(50), None)?;
    Ok(())
}

/// Best-effort removal of a SQLite database file and its WAL sidecars.
fn remove_db_files(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn seed_shared(path: &Path) {
        let conn = open_shared(path).unwrap();
        crate::testkit::set(&conn, "SOR", "Spark of Rebellion");
        crate::testkit::card(&conn, "SOR-1", "Luke Skywalker");
    }

    #[test]
    fn open_shared_creates_schema() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_shared(&dir.path().join("shared.sqlite")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cards'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // Re-opening is idempotent.
        let conn2 = open_shared(&dir.path().join("shared.sqlite")).unwrap();
        let n2: i64 = conn2
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cards'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n2, 1);
    }

    #[test]
    fn attach_exposes_catalog_and_enforces_readonly() {
        let dir = tempfile::tempdir().unwrap();
        let shared_path = dir.path().join("shared.sqlite");
        seed_shared(&shared_path);

        // A fresh in-memory "user" connection with the catalog attached.
        // Apply the user schema too so the FK-existence helpers can see
        // the user_printings table (the "Missing Variant" escape hatch
        // is one of the FK targets `printing_exists` checks).
        let user = Connection::open_in_memory().unwrap();
        init_user_schema(&user).unwrap();
        attach_shared_readonly(&user, &shared_path).unwrap();

        // Catalog tables are reachable unqualified via the temp views.
        let name: String = user
            .query_row("SELECT name FROM sets WHERE set_code = 'SOR'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(name, "Spark of Rebellion");

        // FK-existence helpers see the attached catalog.
        assert!(catalog::card_exists(&user, "SOR-1").unwrap());
        assert!(!catalog::card_exists(&user, "SOR-999").unwrap());
        assert!(!catalog::printing_exists(&user, "SOR-1-normal").unwrap());

        // The attachment is read-only: writing to the catalog fails.
        let write = user.execute(
            "INSERT INTO shared.sets (set_code, name) VALUES ('x', 'y')",
            [],
        );
        assert!(write.is_err(), "shared catalog must be read-only");
    }

    #[test]
    fn restore_is_wal_correct_with_live_connection() {
        // Reproduces dumpstar-lxm: the server holds a long-lived WAL
        // connection; a prior test's write lands in the WAL. A WAL-unaware
        // restore (cp of the main file only) leaves that frame in place, so
        // the next read still sees the mutation. WAL-correct restore must
        // overwrite it.
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("collection.sqlite");

        // The "server" — a persistent connection in WAL mode.
        let server = Connection::open(&live).unwrap();
        server
            .execute_batch(
                "PRAGMA journal_mode = WAL; \
                 CREATE TABLE t (name TEXT PRIMARY KEY, copies INTEGER); \
                 INSERT INTO t VALUES ('Luke', 1);",
            )
            .unwrap();

        // Snapshot the clean state (Luke = 1).
        let bak = dir.path().join("collection.sqlite.bak");
        snapshot_db(&live, &bak).unwrap();

        // A test mutates through the live connection — the write lands in the
        // WAL, not (yet) the main database file.
        server
            .execute("UPDATE t SET copies = 2 WHERE name = 'Luke'", [])
            .unwrap();
        let mutated: i64 = server
            .query_row("SELECT copies FROM t WHERE name = 'Luke'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mutated, 2);

        // Restore, then read back through the *same* live connection.
        restore_db(&bak, &live).unwrap();
        let restored: i64 = server
            .query_row("SELECT copies FROM t WHERE name = 'Luke'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            restored, 1,
            "live connection must see the restored snapshot, not the WAL mutation"
        );
    }

    /// Build a collection database the way the binary that predates the
    /// gate did: schema applied straight, no `user_version` written, rows in
    /// it. This is the shape of every database on disk today, prod's
    /// included — and the shape that took prod down on 2026-08-08, when
    /// every verification was fresh-install shaped and nobody started the
    /// new binary against a volume the old one made.
    fn unversioned_collection(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .unwrap();
        conn.execute_batch(SCHEMA_USER).unwrap();
        conn.execute(
            "INSERT INTO binders (name, created_at, updated_at) \
             VALUES ('Trade Binder', '2026-08-08', '2026-08-08')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source, binder_id) \
             VALUES ('SOR-1-normal', '2026-08-08', 'manual_id', 1)",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(conn);
        assert_eq!(
            file_user_version(path),
            0,
            "the fixture must be genuinely unversioned"
        );
    }

    /// `user_version` straight out of the file header (bytes 60..64,
    /// big-endian), read without opening the database — so the assertion
    /// cannot be satisfied by the very code under test.
    fn file_user_version(path: &Path) -> u32 {
        let bytes = std::fs::read(path).unwrap();
        u32::from_be_bytes(bytes[60..64].try_into().unwrap())
    }

    /// The file change counter (header bytes 24..28), which SQLite bumps on
    /// every write transaction. An unchanged counter is proof that an open
    /// touched nothing.
    fn file_change_counter(path: &Path) -> u32 {
        let bytes = std::fs::read(path).unwrap();
        u32::from_be_bytes(bytes[24..28].try_into().unwrap())
    }

    /// The adoption path, which is the release-blocking one: every database
    /// in existence is version 0, so if this is wrong, prod does not start.
    #[test]
    fn an_unversioned_collection_is_adopted_in_place_with_its_rows() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.sqlite");
        unversioned_collection(&path);
        let before = std::fs::metadata(&path).unwrap().ino();

        let conn = open_user(&path).unwrap();

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM collection", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "adoption must not lose the collection");
        let binder: String = conn
            .query_row("SELECT name FROM binders WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(binder, "Trade Binder");
        drop(conn);

        assert_eq!(
            file_user_version(&path),
            schema_version::User.version as u32,
            "the adopted database must carry the version afterwards"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().ino(),
            before,
            "adoption must happen in place — the file must not be recreated"
        );
    }

    #[test]
    fn an_unversioned_catalog_is_adopted_and_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        seed_shared(&path);
        // Put it back the way the pre-gate binary left it.
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("PRAGMA user_version = 0; PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
        }
        assert_eq!(file_user_version(&path), 0);

        let conn = open_shared(&path).unwrap();
        assert!(crate::catalog::card_exists(&conn, "SOR-1").unwrap());
        drop(conn);
        assert_eq!(
            file_user_version(&path),
            schema_version::Shared.version as u32
        );
    }

    /// Re-opening an up-to-date database leaves its version exactly where it
    /// was — the gate does not creep the number on every start. (That the
    /// stamp writes *nothing at all* in this case is asserted a level down,
    /// in `schema_version`, against the file's change counter.)
    #[test]
    fn re_opening_an_up_to_date_database_writes_no_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.sqlite");
        drop(open_user(&path).unwrap());
        let stamped = file_user_version(&path);
        assert_eq!(stamped, schema_version::User.version as u32);

        for _ in 0..3 {
            let conn = open_user(&path).unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            assert_eq!(file_user_version(&path), stamped);
        }
    }

    /// The gate itself: a collection written by a newer build is refused,
    /// and the refusal names both versions and the file. Rollback is only
    /// safe because of this — an older binary must stop, not quietly
    /// operate on a schema it does not know.
    #[test]
    fn a_collection_from_the_future_is_refused_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let shared_path = dir.path().join("shared.sqlite");
        seed_shared(&shared_path);
        let path = dir.path().join("collection.sqlite");
        drop(open_user(&path).unwrap());

        let ahead = schema_version::User.version + 1;
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&format!("PRAGMA user_version = {ahead}"))
                .unwrap();
        }

        let err = open_user(&path).unwrap_err();
        assert!(matches!(err, crate::error::DbError::SchemaVersion(_)));
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("version {ahead}")),
            "no file version: {msg}"
        );
        assert!(
            msg.contains(&format!("version {}", schema_version::User.version)),
            "no binary version: {msg}"
        );
        assert!(msg.contains("collection.sqlite"), "no file named: {msg}");

        // The whole way in, not just the low-level one.
        assert!(connect_user(&path, &shared_path).is_err());
    }

    /// The server reaches the catalog by attaching it, not through
    /// `open_shared` — so the gate has to be on that path as well.
    #[test]
    fn a_catalog_from_the_future_is_refused_on_attach() {
        let dir = tempfile::tempdir().unwrap();
        let shared_path = dir.path().join("shared.sqlite");
        seed_shared(&shared_path);
        let ahead = schema_version::Shared.version + 1;
        {
            let c = Connection::open(&shared_path).unwrap();
            c.execute_batch(&format!("PRAGMA user_version = {ahead}"))
                .unwrap();
        }

        let err = connect_user(&dir.path().join("collection.sqlite"), &shared_path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("shared.sqlite"), "no file named: {msg}");
        assert!(
            msg.contains(&format!("version {ahead}")),
            "no file version: {msg}"
        );
        assert!(open_shared(&shared_path).is_err(), "and directly, too");
    }

    /// Build a registry the way the binary that predates the gate did:
    /// schema applied straight, no `user_version` written, a real user in it.
    /// Every registry in existence is this shape — the epic that creates the
    /// file and the epic that added the gate were separate branches, so there
    /// has never been a build that stamped one (pd-r60h).
    fn unversioned_registry(path: &Path) -> String {
        let conn = open_registry(path).unwrap();
        let user = crate::registry::insert(&conn, "alice").unwrap();
        conn.execute_batch("PRAGMA user_version = 0").unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(conn);
        assert_eq!(
            file_user_version(path),
            0,
            "the fixture must be genuinely unversioned"
        );
        user.database_id
    }

    /// The adoption path for the third database. It is the one that actually
    /// runs: there is no registry anywhere carrying a version, so if this is
    /// wrong, no box with users on it starts.
    #[test]
    fn an_unversioned_registry_is_adopted_in_place_with_its_rows() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.sqlite");
        let database_id = unversioned_registry(&path);
        let before = std::fs::metadata(&path).unwrap().ino();

        let conn = open_registry(&path).unwrap();

        let user = crate::registry::lookup(&conn, "alice").unwrap().unwrap();
        assert_eq!(
            user.database_id, database_id,
            "adoption must not lose the map from handle to database"
        );
        drop(conn);

        assert_eq!(
            file_user_version(&path),
            schema_version::Registry.version as u32,
            "the adopted registry must carry the version afterwards"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().ino(),
            before,
            "adoption must happen in place — the file must not be recreated"
        );
    }

    /// A registry from the future is refused rather than written to, and the
    /// refusal names both versions and the file.
    ///
    /// Sharper here than for a collection: this is the file that says whose
    /// database is whose. An older build that applied its own schema over a
    /// newer registry would not damage a collection — it would damage the
    /// only thing that can attribute one.
    #[test]
    fn a_registry_from_the_future_is_refused_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.sqlite");
        drop(open_registry(&path).unwrap());

        let ahead = schema_version::Registry.version + 1;
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&format!(
                "PRAGMA user_version = {ahead}; PRAGMA wal_checkpoint(TRUNCATE);"
            ))
            .unwrap();
        }
        let before = file_change_counter(&path);

        let err = open_registry(&path).unwrap_err();
        assert!(matches!(err, crate::error::DbError::SchemaVersion(_)));
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("version {ahead}")),
            "no file version: {msg}"
        );
        assert!(
            msg.contains(&format!("version {}", schema_version::Registry.version)),
            "no binary version: {msg}"
        );
        assert!(msg.contains("registry.sqlite"), "no file named: {msg}");
        assert!(
            msg.contains(schema_version::Registry.label),
            "no database named: {msg}"
        );

        // Refused means not written to. A refusal that had already applied
        // the schema on the way past would be a refusal of the return value
        // only, which is the failure mode the gate exists to prevent.
        assert_eq!(
            file_change_counter(&path),
            before,
            "the refused registry must not have been touched"
        );
        assert_eq!(file_user_version(&path), ahead as u32);
    }

    /// All THREE databases are gated, not two. The registry was declared in
    /// `schema_version` and wired to nothing for as long as the two epics were
    /// separate branches, and "declared" is not "enforced" (pd-r60h).
    #[test]
    fn every_database_refuses_a_file_from_the_future() {
        let dir = tempfile::tempdir().unwrap();
        let ahead_by_one = |path: &Path, db: dumpster_kit_tenancy::SchemaSpec| {
            let c = Connection::open(path).unwrap();
            c.execute_batch(&format!("PRAGMA user_version = {}", db.version + 1))
                .unwrap();
        };

        let shared = dir.path().join("shared.sqlite");
        let user = dir.path().join("collection.sqlite");
        let registry = dir.path().join("registry.sqlite");
        drop(open_shared(&shared).unwrap());
        drop(open_user(&user).unwrap());
        drop(open_registry(&registry).unwrap());
        ahead_by_one(&shared, schema_version::Shared);
        ahead_by_one(&user, schema_version::User);
        ahead_by_one(&registry, schema_version::Registry);

        assert!(open_shared(&shared).is_err(), "the catalog is not gated");
        assert!(open_user(&user).is_err(), "the collection is not gated");
        assert!(
            open_registry(&registry).is_err(),
            "the registry is not gated"
        );
    }

    /// The migration-history table the pre-luo migration system left on
    /// every database built before it was removed. Recreated here in the
    /// shape refinery wrote it so the drop is exercised against the real
    /// thing rather than an empty stand-in.
    fn add_legacy_refinery_table(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE refinery_schema_history ( \
                 version INT4 PRIMARY KEY, \
                 name VARCHAR(255), \
                 applied_on VARCHAR(255), \
                 checksum VARCHAR(255)); \
             INSERT INTO refinery_schema_history \
                 VALUES (1, 'initial', '2026-05-18T00:00:00', 'deadbeef');",
        )
        .unwrap();
    }

    fn has_table(conn: &Connection, name: &str) -> bool {
        conn.prepare("SELECT 1 FROM main.sqlite_master WHERE type='table' AND name=?1")
            .unwrap()
            .exists([name])
            .unwrap()
    }

    /// pd-yj40: the legacy table goes away on open, so no reader downstream
    /// has to know its name. Before the drop the JSON export was the one
    /// keeping it out of a fresh collection — by naming it.
    #[test]
    fn a_legacy_refinery_table_is_dropped_from_a_collection_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.sqlite");
        unversioned_collection(&path);
        add_legacy_refinery_table(&path);

        let conn = open_user(&path).unwrap();
        assert!(
            !has_table(&conn, "refinery_schema_history"),
            "the legacy migration table must not survive an open"
        );
        // ...and the collection it sat beside is untouched.
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM collection", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        // The exporter no longer names it, so this is what keeps it out.
        assert!(
            !crate::json_backup::user_tables(&conn)
                .unwrap()
                .iter()
                .any(|t| t == "refinery_schema_history"),
            "a dropped table cannot reach the JSON envelope"
        );
    }

    /// The catalog carried its own copy, in the files the fork's Pokémon
    /// schema wrote. Those files are emptied outright by `open_shared`
    /// (`dumpstar setup`, the offline derive, the server's own startup — the
    /// paths that hold it read-write), so the table goes with everything else
    /// and the schema no longer needs a `DROP` for it: the Star Wars: Unlimited
    /// schema never had one. A connection that merely attaches the catalog, or
    /// `open_shared_readonly`, cannot do this.
    #[test]
    fn a_legacy_refinery_table_does_not_survive_the_catalog_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        legacy_pokemon_catalog(&path);
        add_legacy_refinery_table(&path);

        let conn = open_shared(&path).unwrap();
        assert!(!has_table(&conn, "refinery_schema_history"));
        crate::testkit::set(&conn, "SOR", "Spark of Rebellion");
        crate::testkit::card(&conn, "SOR-1", "Luke Skywalker");
        assert!(crate::catalog::card_exists(&conn, "SOR-1").unwrap());
    }

    /// The drop is a statement in the schema, which is re-applied on every
    /// single open — so it must cost nothing once there is nothing to drop.
    /// A write here would be a write on every server start, replicated
    /// off-box by Litestream each time (same standard as the version stamp).
    #[test]
    fn dropping_a_table_that_is_already_gone_writes_nothing_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.sqlite");
        {
            let conn = open_user(&path).unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
        }
        let before = file_change_counter(&path);

        for _ in 0..3 {
            let conn = open_user(&path).unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            assert_eq!(
                file_change_counter(&path),
                before,
                "re-opening must not write to the database"
            );
        }
    }

    /// A collection written before `conditions` moved into the user schema
    /// (pd-s4c2) — the restored-from-an-old-replica case, and the one
    /// per-file versioning makes genuinely reachable.
    fn pre_move_collection(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .unwrap();
        conn.execute_batch(SCHEMA_USER).unwrap();
        // Put it back the way the previous build left it: no `conditions`,
        // carrying that build's user_version.
        conn.execute_batch("DROP TABLE conditions; PRAGMA user_version = 1;")
            .unwrap();
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source, condition) \
             VALUES ('SOR-1-normal', '2026-08-08', 'manual_id', 'Lightly Played')",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(conn);
        assert_eq!(file_user_version(path), 1, "the fixture must be pre-move");
    }

    /// The move IS the migration: an existing collection grows the table and
    /// its five defaults on the next open, keeping its rows, and comes out
    /// stamped with this build's version.
    #[test]
    fn a_collection_written_before_the_move_grows_conditions_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.sqlite");
        pre_move_collection(&path);

        let conn = open_user(&path).unwrap();
        assert!(has_table(&conn, "conditions"));
        let m = crate::conditions::multipliers(&conn).unwrap();
        assert_eq!(m.len(), 5);
        assert_eq!(m.get("Lightly Played"), Some(&0.85));
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM collection", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the migration must not lose the collection");
        drop(conn);

        assert_eq!(
            file_user_version(&path),
            schema_version::User.version as u32
        );
    }

    /// The catalog sheds its copy on the one path that holds it read-write:
    /// the rebuild of a Pokémon-shaped file takes every table with it.
    #[test]
    fn the_catalogs_conditions_table_does_not_survive_the_catalog_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        legacy_pokemon_catalog(&path);
        add_stale_catalog_conditions(&path);

        let conn = open_shared(&path).unwrap();
        assert!(!has_table(&conn, "conditions"));
        crate::testkit::set(&conn, "SOR", "Spark of Rebellion");
        crate::testkit::card(&conn, "SOR-1", "Luke Skywalker");
        assert!(catalog::card_exists(&conn, "SOR-1").unwrap());
    }

    /// A catalog built before the move, still physically carrying the table.
    /// `0.01` so a multiplier read from here is unmistakable.
    fn add_stale_catalog_conditions(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE conditions ( \
                 name TEXT PRIMARY KEY, multiplier REAL NOT NULL, rank INTEGER NOT NULL); \
             INSERT INTO conditions VALUES ('Near Mint', 0.01, 0);",
        )
        .unwrap();
    }

    /// The catalog must never shade the collection's own tables. SQLite
    /// resolves an unqualified name in `temp` before `main`, and
    /// `gate_attached` deliberately accepts a catalog that is *behind* this
    /// build — so a `shared.sqlite` not yet through `dumpstar setup` since the
    /// move still holds `conditions`, and a TEMP VIEW over it would silently
    /// become the multipliers every value on the page is computed from.
    #[test]
    fn a_stale_catalog_table_does_not_shade_the_collections_own() {
        let dir = tempfile::tempdir().unwrap();
        let shared_path = dir.path().join("shared.sqlite");
        seed_shared(&shared_path);
        add_stale_catalog_conditions(&shared_path);

        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared_path).unwrap();
        let m = crate::conditions::multipliers(&conn).unwrap();
        assert_eq!(
            m.get("Near Mint"),
            Some(&1.0),
            "the collection's own multiplier must win over the catalog's"
        );
        assert_eq!(
            m.len(),
            5,
            "and the collection's whole seed must be visible"
        );
        // The catalog is still reachable when asked for by name — this is a
        // resolution rule, not a hidden table.
        let stale: f64 = conn
            .query_row("SELECT multiplier FROM shared.conditions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stale, 0.01);
    }

    #[test]
    fn connect_user_attaches_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let shared_path = dir.path().join("shared.sqlite");
        seed_shared(&shared_path);

        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared_path).unwrap();
        assert!(catalog::card_exists(&conn, "SOR-1").unwrap());
    }

    #[test]
    fn connect_user_has_user_schema_and_enforces_exclusivity() {
        let dir = tempfile::tempdir().unwrap();
        let shared_path = dir.path().join("shared.sqlite");
        seed_shared(&shared_path);
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared_path).unwrap();

        conn.execute(
            "INSERT INTO binders (name, created_at, updated_at) \
             VALUES ('Trade Binder', '2026-05-18', '2026-05-18')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO decks (name, created_at, updated_at) \
             VALUES ('Alice''s deck', '2026-05-18', '2026-05-18')",
            [],
        )
        .unwrap();

        // A card may sit in a binder OR a deck.
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source, binder_id) \
             VALUES ('SOR-1-normal', '2026-05-18', 'manual_id', 1)",
            [],
        )
        .unwrap();

        // ...but not both — the exclusivity CHECK rejects it.
        let both = conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source, binder_id, deck_id) \
             VALUES ('SOR-1-foil', '2026-05-18', 'manual_id', 1, 1)",
            [],
        );
        assert!(
            both.is_err(),
            "a card cannot be in a binder and a deck at once"
        );
    }

    fn legacy_pokemon_catalog(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        conn.execute_batch(dumpstar_catalog::LEGACY_SHAPE_DDL)
            .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }

    /// sp-r7w5xi: a `shared.sqlite` left by the Pokémon schema is derived
    /// data; opening it read-write empties it and builds the one card model on
    /// top, rather than failing on a column that is not there.
    #[test]
    fn open_shared_over_a_pokemon_shaped_file_empties_and_rebuilds_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        legacy_pokemon_catalog(&path);
        assert_eq!(file_user_version(&path), 2);

        let conn = open_shared(&path).unwrap();
        for gone in dumpstar_catalog::LEGACY_ONLY_OBJECTS {
            assert!(!has_table(&conn, gone), "{gone} survived the rebuild");
        }
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cards')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            cols.contains(&"aspects".to_string()),
            "not the SWU cards table"
        );
        assert!(
            !dumpstar_catalog::is_legacy(&conn).unwrap(),
            "a legacy column survived"
        );
        assert_eq!(count(&conn, "sets"), 0, "the legacy rows are emptied");
        assert_eq!(count(&conn, "cards"), 0);
        assert!(
            has_table(&conn, "sealed_products"),
            "the overlay is applied"
        );
        assert!(
            crate::convergence::is_converged(&conn),
            "the rebuilt file records this build's fingerprint"
        );
        drop(conn);
        assert_eq!(file_user_version(&path), 3);
        assert_eq!(
            file_user_version(&path),
            schema_version::Shared.version as u32
        );
    }

    /// The rebuild happens once: the file it leaves is SWU-shaped, so the next
    /// open keeps whatever the catalog apply has put in since.
    #[test]
    fn a_second_open_shared_keeps_rows_written_after_the_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        legacy_pokemon_catalog(&path);

        let conn = open_shared(&path).unwrap();
        crate::testkit::simple(&conn, "SOR", "1", "Luke Skywalker");
        crate::testkit::price(&conn, "SOR-1-normal", "2026-10-01", 1.5);
        drop(conn);

        let conn = open_shared(&path).unwrap();
        assert_eq!(count(&conn, "sets"), 1);
        assert_eq!(count(&conn, "cards"), 1);
        assert_eq!(count(&conn, "printings"), 1);
        assert_eq!(count(&conn, "prices"), 1);
    }

    /// `backfill_sealed` folds tcgcsv products with no collector number into
    /// `sealed_products`, once: a second pass (every open runs one) adds
    /// nothing and does not chase a changed name.
    #[test]
    fn backfill_sealed_copies_numberless_tcgcsv_products_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        let conn = open_shared(&path).unwrap();
        crate::testkit::set(&conn, "SOR", "Spark of Rebellion");
        conn.execute_batch(
            "INSERT INTO tcgcsv_groups (group_id, name, abbreviation) \
               VALUES (23405, 'Spark of Rebellion', 'SOR'); \
             INSERT INTO tcgcsv_products (product_id, group_id, name, number) VALUES \
               (1001, 23405, 'Spark of Rebellion Booster Box', NULL), \
               (1002, 23405, 'Spark of Rebellion Booster Pack', NULL), \
               (1003, 23405, 'Luke Skywalker', '005');",
        )
        .unwrap();
        assert_eq!(count(&conn, "sealed_products"), 0);

        backfill_sealed(&conn).unwrap();
        assert_eq!(
            count(&conn, "sealed_products"),
            2,
            "only the numberless products"
        );
        let (set_code, category): (Option<String>, String) = conn
            .query_row(
                "SELECT set_code, category FROM sealed_products WHERE product_id = 1001",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(set_code.as_deref(), Some("SOR"));
        assert_eq!(category, "sealed");

        // Again, with the catalog's name having moved: nothing is added and
        // nothing is rewritten.
        conn.execute(
            "UPDATE tcgcsv_products SET name = 'Renamed' WHERE product_id = 1001",
            [],
        )
        .unwrap();
        backfill_sealed(&conn).unwrap();
        assert_eq!(count(&conn, "sealed_products"), 2);
        let name: String = conn
            .query_row(
                "SELECT name FROM sealed_products WHERE product_id = 1001",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "Spark of Rebellion Booster Box");
        drop(conn);

        // An ordinary open runs it too, with the same outcome.
        let conn = open_shared(&path).unwrap();
        assert_eq!(count(&conn, "sealed_products"), 2);
    }

    /// An ordinary restart of the server asks one read-only question and is
    /// done: no write lock, no write to the file (pd-dzu5).
    #[test]
    fn open_shared_for_serving_on_a_converged_file_takes_the_read_only_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        let keep_shm = open_shared(&path).unwrap();
        keep_shm
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        let before = file_change_counter(&path);

        let conn = open_shared_for_serving(&path).unwrap();
        assert!(
            conn.is_readonly(rusqlite::DatabaseName::Main).unwrap(),
            "a converged catalog is served from a read-only handle"
        );
        assert!(conn.execute("DELETE FROM sets", []).is_err());
        assert_eq!(file_change_counter(&path), before, "nothing was written");

        // A file that is not converged falls through to the read-write open.
        let stale = dir.path().join("stale.sqlite");
        let c = open_shared(&stale).unwrap();
        c.execute_batch("DELETE FROM catalog_convergence").unwrap();
        let conn = open_shared_for_serving(&stale).unwrap();
        assert!(!conn.is_readonly(rusqlite::DatabaseName::Main).unwrap());
        assert!(crate::convergence::is_converged(&conn));
    }
}
