//! A catalog derived from `raw/` is the catalog a FETCHING pass built from the
//! responses `raw/` holds — **row by row**.
//!
//! Both sides of every comparison here come from one pass over one set of
//! upstream responses. The fetching side fetches them and lands them; the
//! offline side replays the bytes that landing kept. Fetching twice would make
//! "the two catalogs disagree" and "the upstream answered differently"
//! indistinguishable, which is the whole thing the gate exists to tell apart.
//!
//! The fetching side is this harness, not a shipped command: `dumpstar data
//! refresh` lands and derives nothing, and [`Harness::online`] is the half that
//! also builds a catalog.
//!
//! ## What is real here and what is a stand-in
//!
//! | | |
//! | --- | --- |
//! | the derivation | real — [`dumpstar_derive::derive`], the function `dumpstar-lake-derive shared` calls |
//! | the landing zone | real — `RawLanding` over a `DirStore`, the same writer prod uses over S3 |
//! | the reader | real — the shipped `dumpstar-lake-derive` binary, run as a subprocess |
//! | the comparison | real — `dumpstar-lake-derive diff`, the shipped comparator |
//! | the upstream | a fixture: an in-process HTTP server, so the run is hermetic |
//!
//! The offline half runs as a **subprocess of the real binary**, deliberately.
//! It is what a timer executes, its refusals are exit statuses rather than
//! `Result`s, and its loudness is text on a stream.
//!
//! ## The upstream stays UP, on purpose
//!
//! The fixture is a socket on loopback and it answers throughout every test
//! here, including the ones that assert a refusal. That is deliberate: a gap in
//! `raw/` is fatal by policy, and a gate run with nothing listening could not
//! tell policy from a failed connection.
//!
//! ## What is not here yet
//!
//! The Star Wars: Unlimited catalog tables (design row S6). Until the catalog
//! seam lands, a derive builds no card rows, so what is compared is the
//! materialised prices and the provenance — and the refusals, which are the
//! part of this file that does not depend on a catalog at all.

// The fake upstream is the one `dumpstar-ingest`'s landing-zone gates already
// drive the real clients with. A second HTTP server would be a second thing
// that can be subtly wrong.
#[allow(dead_code)]
#[path = "../../dumpstar-ingest/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use dumpstar_derive::DeriveClock;
use dumpstar_lake::{DirStore, RawLanding};
use support::{FakeUpstream, Reply};

/// The first day's partition, and the clock the run that landed it read.
const DAY1: &str = "2026-08-10";
const DAY1_CLOCK: &str = "2026-08-10T06:14:07+00:00";
/// The second day. Its prices differ, so its rows must too.
const DAY2: &str = "2026-08-11";
const DAY2_CLOCK: &str = "2026-08-11T06:12:55+00:00";

/// `raw_derivation` is written by the offline job and by nothing else — the
/// online refresh fetched rather than replayed, so it has no run to name. It
/// is the one table that legitimately differs, and every comparison below
/// names it on the command line rather than skipping it quietly.
const PROVENANCE: &str = "raw_derivation";

/// The upstream's clients read their origin from the process environment, so
/// the online half of a comparison cannot run beside another one.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// The fixture upstream
// ---------------------------------------------------------------------------

const GROUPS: &str = r#"{"success":true,"errors":[],"results":[
  {"groupId":1,"name":"Fixture Set One","abbreviation":"FS1","publishedOn":"2026-01-09T00:00:00"},
  {"groupId":2,"name":"Fixture Set Two","abbreviation":"FS2","publishedOn":"2026-06-16T00:00:00"}
]}"#;

fn products_json(group: i64) -> String {
    format!(
        r#"{{"success":true,"errors":[],"results":[
          {{"productId":{p1},"groupId":{group},"name":"Fixture Leader - 001/002","imageCount":1,
            "extendedData":[{{"name":"Number","value":"001/002"}}]}},
          {{"productId":{p2},"groupId":{group},"name":"Fixture Booster Box","imageCount":1,
            "extendedData":[]}}
        ]}}"#,
        p1 = group * 100 + 1,
        p2 = group * 100 + 2,
    )
}

/// Prices for one group on one day. Every quoted value moves with the day, so
/// a catalog built from the wrong day's parts is wrong in its *numbers* and
/// not merely in its bookkeeping.
fn prices_json(group: i64, day: usize) -> String {
    let d = day as f64;
    format!(
        r#"{{"success":true,"errors":[],"results":[
          {{"productId":{p1},"subTypeName":"Normal","lowPrice":{:.2},"midPrice":{:.2},
            "highPrice":{:.2},"marketPrice":{:.2},"directLowPrice":null}},
          {{"productId":{p2},"subTypeName":"Normal","lowPrice":null,"midPrice":null,
            "highPrice":null,"marketPrice":{:.2},"directLowPrice":null}}
        ]}}"#,
        1.0 + d,
        2.0 + d,
        3.0 + d,
        2.5 + d,
        120.0 + d,
        p1 = group * 100 + 1,
        p2 = group * 100 + 2,
    )
}

/// The upstream both halves of a comparison read from. `day` selects which
/// day's payloads it serves; `fail_group` makes one `/prices` request return a
/// 503, which is how a run that dies partway is produced.
struct Script {
    day: AtomicUsize,
    fail_group: AtomicUsize,
}

fn start_upstream(script: Arc<Script>) -> FakeUpstream {
    FakeUpstream::start(move |target, _n| {
        let day = script.day.load(Ordering::SeqCst);
        let (path, _query) = target.split_once('?').unwrap_or((target, ""));
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        match parts.as_slice() {
            [_, "groups"] => Reply::ok(GROUPS),
            [_, group, "products"] => Reply::ok(products_json(group.parse().unwrap())),
            [_, group, "prices"] => {
                let group: usize = group.parse().unwrap();
                if script.fail_group.load(Ordering::SeqCst) == group {
                    return Reply::status(503, r#"{"error":"upstream is having a day"}"#);
                }
                Reply::ok(prices_json(group as i64, day))
            }
            _ => Reply::status(404, format!(r#"{{"error":"no route for {target}"}}"#)),
        }
    })
}

// ---------------------------------------------------------------------------
// The two halves
// ---------------------------------------------------------------------------

/// A working directory with a landing zone, catalogs and an upstream.
struct Harness {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    upstream: FakeUpstream,
    script: Arc<Script>,
}

impl Harness {
    fn start() -> Self {
        Self::at(None)
    }

    /// A harness whose working directory is `out` rather than a temp dir —
    /// how `tests/lake/derive.sh` gets a landing zone produced by the REAL
    /// writer instead of a shell's idea of the key layout. The temp dir is
    /// still created and still cleans up; it is simply unused.
    fn at(out: Option<PathBuf>) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = out.unwrap_or_else(|| tmp.path().to_path_buf());
        std::fs::create_dir_all(&root).expect("create the working directory");
        let script = Arc::new(Script {
            day: AtomicUsize::new(1),
            fail_group: AtomicUsize::new(0),
        });
        let upstream = start_upstream(Arc::clone(&script));
        Self {
            _tmp: tmp,
            root,
            upstream,
            script,
        }
    }

    fn raw(&self) -> PathBuf {
        self.root.join("raw-zone")
    }

    fn db(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.sqlite"))
    }

    fn day(&self, day: usize) {
        self.script.day.store(day, Ordering::SeqCst);
    }

    /// The ONLINE path: fetch from the fixture upstream, land every response,
    /// derive into `db`. `fail` makes the acquisition phase die partway, which
    /// is how the incomplete partition the refusal test needs gets landed.
    fn online(&self, db: &Path, ingest_date: &str, clock_at: &str, fail: bool) -> bool {
        let clock = DeriveClock::from_manifest(clock_at, "the test's clock").expect("clock");
        let landing = Arc::new(RawLanding::new(
            Box::new(DirStore::new(self.raw())),
            ingest_date,
            clock.fetched_at(),
        ));
        self.fetch_and_derive(db, clock, Some(landing), fail)
    }

    /// A derivation that fetches and lands NOTHING — the reference catalog for
    /// the landing-only gate.
    fn online_unlanded(&self, db: &Path, clock_at: &str) -> bool {
        let clock = DeriveClock::from_manifest(clock_at, "the test's clock").expect("clock");
        self.fetch_and_derive(db, clock, None, false)
    }

    fn fetch_and_derive(
        &self,
        db: &Path,
        clock: DeriveClock,
        landing: Option<Arc<RawLanding>>,
        fail: bool,
    ) -> bool {
        let _guard = self.env_lock();
        self.script
            .fail_group
            .store(if fail { 2 } else { 0 }, Ordering::SeqCst);

        let mut conn = dumpstar_db::open_shared(db).expect("open shared catalog");
        let outcome = dumpstar_derive::derive(
            &mut conn,
            &dumpstar_derive::Options {
                clock,
                data_dir: &self.root,
                landing,
                replay: None,
            },
        );
        self.script.fail_group.store(0, Ordering::SeqCst);
        outcome.is_ok()
    }

    /// The ONLINE path as `dumpstar data refresh` runs it: fetch every
    /// upstream, land every response, and derive nothing at all.
    fn land(&self, ingest_date: &str, clock_at: &str) {
        let _guard = self.env_lock();
        let clock = DeriveClock::from_manifest(clock_at, "the test's clock").expect("clock");
        let landing = Arc::new(RawLanding::new(
            Box::new(DirStore::new(self.raw())),
            ingest_date,
            clock.fetched_at(),
        ));
        dumpstar_derive::land(&dumpstar_derive::Options {
            clock,
            data_dir: &self.root,
            landing: Some(landing),
            replay: None,
        })
        .expect("the landing run");
    }

    /// The OFFLINE job, as shipped: the real binary, its own process, its own
    /// environment. Returns its exit status and both streams.
    fn derive(&self, db: &Path, ingest_date: &str, extra: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dumpstar-lake-derive"));
        cmd.args(["shared", "--ingest-date", ingest_date])
            .arg("--db")
            .arg(db)
            .arg("--data-dir")
            .arg(&self.root)
            .args(extra)
            .env("DUMPSTAR_LAKE_DIR", self.raw())
            // The landing zone is keyed by URL, so the replaying side has to
            // build the same URLs the fetching side did. In production that is
            // automatic — both are the compiled-in constant. Here the fixture
            // upstream's port is ephemeral, so it has to be passed along.
            .env("DUMPSTAR_TCGCSV_BASE_URL", self.upstream.base_url())
            // Never read: the lake is `DUMPSTAR_LAKE_DIR` above. Pointed at a
            // path that does not exist so a stray read of the operator's real
            // lake.env would fail loudly rather than pass silently.
            .env("DUMPSTAR_LAKE_ENV", self.root.join("no-such-lake.env"));
        cmd.output().expect("run dumpstar-lake-derive")
    }

    /// `dumpstar-lake-derive diff`, the shipped comparator, excluding only the
    /// provenance table the online path never writes.
    fn diff(&self, left: &Path, right: &Path) -> Output {
        self.diff_excluding(left, right, &[PROVENANCE])
    }

    /// As [`Harness::diff`], with the exclusions spelled out. Every caller
    /// that needs more than the provenance table has to say which and why —
    /// an exclusion nobody can see is how a comparison starts proving nothing.
    fn diff_excluding(&self, left: &Path, right: &Path, exclude: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dumpstar-lake-derive"));
        cmd.arg("diff")
            .arg("--left")
            .arg(left)
            .arg("--right")
            .arg(right);
        for table in exclude {
            cmd.args(["--exclude", table]);
        }
        cmd.output().expect("run dumpstar-lake-derive diff")
    }

    fn env_lock(&self) -> MutexGuard<'_, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: every writer of these variables in this binary holds the
        // lock above, and the offline half runs in its own process.
        unsafe {
            std::env::set_var("DUMPSTAR_TCGCSV_BASE_URL", self.upstream.base_url());
        }
        guard
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A single value out of a catalog, for the assertions that are about one
/// specific row rather than about the whole file.
fn scalar<T: rusqlite::types::FromSql>(db: &Path, sql: &str) -> T {
    let conn = rusqlite::Connection::open(db).expect("open");
    conn.query_row(sql, [], |r| r.get(0)).expect("query")
}

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// **The acceptance criterion.** Land a day through the online path, rebuild
/// it from what landed, and compare the two catalogs row for row.
#[test]
fn a_catalog_derived_from_raw_is_row_identical_to_the_online_one() {
    let h = Harness::start();
    let online = h.db("online");
    let offline = h.db("offline");

    assert!(h.online(&online, DAY1, DAY1_CLOCK, false), "online refresh");

    let out = h.derive(&offline, DAY1, &[]);
    assert!(
        out.status.success(),
        "offline derive failed:\n{}",
        text(&out)
    );
    // "It succeeded" already means every request was answered from raw/ — a
    // single miss is fatal. The line is asserted as well, because it is what
    // an operator reads.
    assert!(
        text(&out).contains("raw coverage: complete"),
        "the derive must SAY the partition covered it:\n{}",
        text(&out)
    );
    // Provenance names the three TCGCSV datasets it read.
    assert_eq!(
        scalar::<i64>(&offline, "SELECT COUNT(*) FROM raw_derivation"),
        3
    );

    let diff = h.diff(&online, &offline);
    assert!(
        diff.status.success(),
        "the two catalogs are not row-identical:\n{}",
        text(&diff)
    );
    assert!(text(&diff).contains("ROW-IDENTICAL"));
}

/// The catalog has ONE builder: the refresh lands and nothing else, so the
/// partition it leaves has to stand on its own.
#[test]
fn a_catalog_derived_from_a_landing_only_refresh_is_row_identical_to_a_fetched_one() {
    let h = Harness::start();
    let offline = h.db("offline");
    let reference = h.db("reference");

    h.land(DAY1, DAY1_CLOCK);

    let out = h.derive(&offline, DAY1, &[]);
    assert!(
        out.status.success(),
        "the partition a landing-only refresh left is not derivable:\n{}",
        text(&out)
    );
    assert!(
        text(&out).contains("raw coverage: complete"),
        "a landing-only refresh must land every URL the derive asks for:\n{}",
        text(&out)
    );

    assert!(
        h.online_unlanded(&reference, DAY1_CLOCK),
        "the reference derivation"
    );
    let diff = h.diff(&reference, &offline);
    assert!(
        diff.status.success(),
        "a catalog built from a landing-only refresh differs from a fetched one:\n{}",
        text(&diff)
    );
    assert!(text(&diff).contains("ROW-IDENTICAL"));
}

/// The same claim on the SECOND pass, with different numbers.
#[test]
fn the_second_day_is_row_identical_too() {
    let h = Harness::start();
    let online = h.db("online");
    let offline = h.db("offline");

    h.day(1);
    assert!(h.online(&online, DAY1, DAY1_CLOCK, false));
    h.day(2);
    assert!(h.online(&online, DAY2, DAY2_CLOCK, false));

    // The offline catalog replays the same two dates, in the same order.
    assert!(h.derive(&offline, DAY1, &[]).status.success());
    let out = h.derive(&offline, DAY2, &[]);
    assert!(out.status.success(), "{}", text(&out));

    let diff = h.diff(&online, &offline);
    assert!(
        diff.status.success(),
        "the second day is not row-identical:\n{}",
        text(&diff)
    );
}

/// Deriving the same date twice changes nothing but the provenance, and the
/// provenance says it happened.
#[test]
fn deriving_the_same_date_twice_changes_nothing_but_says_it_happened() {
    let h = Harness::start();
    let once = h.db("once");
    let twice = h.db("twice");

    assert!(h.online(&h.db("online"), DAY1, DAY1_CLOCK, false));
    assert!(h.derive(&twice, DAY1, &[]).status.success());

    // A byte copy of the catalog after ONE derive, to compare the second
    // derive's output against.
    std::fs::copy(&twice, &once).expect("snapshot the catalog");
    let first_derived_at: String = scalar(&once, "SELECT MIN(derived_at) FROM raw_derivation");
    let runs_before: i64 = scalar(&once, "SELECT COUNT(*) FROM raw_derivation");
    assert!(runs_before > 0, "the first derive recorded its provenance");

    assert!(h.derive(&twice, DAY1, &[]).status.success());

    let diff = h.diff_excluding(&once, &twice, &[PROVENANCE]);
    assert!(
        diff.status.success(),
        "a second derive of the same date changed the catalog:\n{}",
        text(&diff)
    );

    // …and the provenance says a derive ran again, without accumulating.
    assert_eq!(
        scalar::<i64>(&twice, "SELECT COUNT(*) FROM raw_derivation"),
        runs_before,
        "delete-then-insert per ingest_date — a rerun replaces, never appends"
    );
    assert_ne!(
        scalar::<String>(&twice, "SELECT MIN(derived_at) FROM raw_derivation"),
        first_derived_at,
        "a rerun must be IDENTIFIABLE, not invisible"
    );
    assert_eq!(
        scalar::<String>(&twice, "SELECT DISTINCT observed_at FROM raw_derivation"),
        DAY1,
        "the observation day comes from the run's clock"
    );
}

/// Rebuilding an OLDER date produces that date's catalog, not today's.
///
/// The failure this is aimed at is the reverse of a rerun: a job that reached
/// for the newest partition when asked for an older one would look like it
/// worked, and would quietly file the wrong day's prices.
#[test]
fn an_older_date_rebuilds_that_date_and_not_the_newest_one() {
    let h = Harness::start();
    let day1_only = h.db("day1-online");
    let both_days = h.db("both-online");

    // Land both days, and keep a catalog that only ever saw day one.
    h.day(1);
    assert!(h.online(&day1_only, DAY1, DAY1_CLOCK, false));
    h.day(2);
    assert!(h.online(&both_days, DAY2, DAY2_CLOCK, false));

    // Now rebuild the OLDER date, with the newer one sitting in raw/ beside it.
    let rebuilt = h.db("rebuilt");
    let out = h.derive(&rebuilt, DAY1, &[]);
    assert!(out.status.success(), "{}", text(&out));

    let diff = h.diff(&day1_only, &rebuilt);
    assert!(
        diff.status.success(),
        "rebuilding {DAY1} did not reproduce {DAY1}:\n{}",
        text(&diff)
    );
    assert_eq!(
        scalar::<String>(&rebuilt, "SELECT DISTINCT observed_at FROM raw_derivation"),
        DAY1,
        "a rebuild of {DAY1} must say it read {DAY1}"
    );
}

/// A date whose landing died partway is refused, not derived: `products` with
/// 200 groups of an unknown 450 is precisely the quietly-smaller catalog the
/// refusal exists for, and it is the half of a night that no later run can
/// re-fetch.
#[test]
fn an_incomplete_partition_is_refused_rather_than_half_derived() {
    let h = Harness::start();
    // Fails on group 2's prices, so the run lands part of the day and stops.
    assert!(
        !h.online(&h.db("online"), DAY1, DAY1_CLOCK, true),
        "the fixture upstream was supposed to fail this run"
    );

    let target = h.db("from-incomplete");
    let out = h.derive(&target, DAY1, &[]);
    assert!(
        !out.status.success(),
        "an incomplete partition must not derive:\n{}",
        text(&out)
    );
    let said = text(&out);
    assert!(said.contains("no complete run"), "{said}");
    assert!(said.contains(DAY1), "{said}");
    // And it refused BEFORE writing anything: a refusal that left a partial
    // catalog behind would be the failure it exists to prevent, one step later.
    assert!(
        !target.exists(),
        "the refusal must not leave a catalog behind"
    );
}

/// A gap in `raw/` is FATAL, and it is fatal by default.
///
/// A URL is removed from the landing zone, which is what raw coverage
/// regressing looks like from the derive's side: the derivation grew an input
/// the landing zone does not capture, or an upstream's origin moved. The run
/// must stop there and say what to do about it.
///
/// This test used to assert the temporary fallback in both directions — loud
/// with it on, fatal with `--no-upstream-fallback`. Item 4 deleted the
/// fallback, so there is one direction left, and the extra assertion below is
/// what makes that stick: **the upstream is up and reachable throughout**. The
/// fixture is a socket on loopback that would have answered. A future edit that
/// reintroduced a fallback would make this run succeed rather than merely
/// change its wording, so the test would fail rather than rot.
///
/// The flag itself is asserted gone too. Leaving it accepted-but-ignored would
/// be worse than removing it: `deploy/derive.sh` invocations and runbooks
/// carrying it would keep passing while meaning nothing.
#[test]
fn a_gap_in_raw_is_fatal_with_the_upstream_sitting_right_there() {
    let h = Harness::start();
    assert!(h.online(&h.db("online"), DAY1, DAY1_CLOCK, false));

    // Delete one landed payload — group 2's prices. The manifest still lists
    // it, so this is a lake that lost an object rather than a run that never
    // fetched one... which `payload`'s digest check would catch. Instead,
    // rewrite the manifest so the URL is genuinely absent from the partition:
    // the "we stopped landing this endpoint" shape.
    let manifest = find_manifest(&h.raw(), "dataset=prices");
    let text_before = std::fs::read_to_string(&manifest).expect("read manifest");
    let mut parsed: dumpstar_lake::Manifest =
        serde_json::from_str(&text_before).expect("parse manifest");
    let dropped = parsed
        .parts
        .iter()
        .position(|p| p.url.contains("/2/prices"))
        .expect("group 2's prices are in the manifest");
    let dropped_url = parsed.parts.remove(dropped).url;
    std::fs::write(&manifest, parsed.to_json().expect("re-serialize")).expect("write manifest");

    let target = h.db("gap");
    let out = h.derive(&target, DAY1, &[]);
    assert!(
        !out.status.success(),
        "a gap in raw/ must be fatal, with no flag asked for:\n{}",
        text(&out)
    );
    let said = text(&out);
    assert!(said.contains("raw/ has no record of"), "{said}");
    assert!(said.contains(&dropped_url), "it must name the URL:\n{said}");
    assert!(said.contains("dumpstar data refresh"), "{said}");

    // Not "the network happened to be down". The fixture upstream is on
    // loopback and answering — the run before this one fetched every one of
    // these URLs from it. The refusal is policy, not circumstance.
    assert!(
        h.upstream
            .requests()
            .iter()
            .any(|r| r.contains("/2/prices")),
        "the fixture upstream never served group 2's prices, so this test would \
         pass against a fallback that simply could not reach anything"
    );

    // And the opt-out is gone rather than tolerated.
    let out = h.derive(&h.db("flag"), DAY1, &["--no-upstream-fallback"]);
    assert!(
        !out.status.success(),
        "--no-upstream-fallback must be rejected, not silently accepted:\n{}",
        text(&out)
    );
    assert!(
        text(&out).contains("unexpected argument"),
        "clap should refuse the retired flag by name:\n{}",
        text(&out)
    );
}

/// A derivation that goes wrong must fail the job, not ship a smaller catalog.
///
/// The induced failure is a payload that no longer hashes to what its manifest
/// recorded — the shape a corrupted or tampered lake has. It is the case where
/// a silent wrong answer would be worst: the bytes still parse, the rows still
/// look like rows, and nothing downstream could tell.
#[test]
fn a_corrupted_payload_stops_the_derive_instead_of_deriving_from_it() {
    let h = Harness::start();
    assert!(h.online(&h.db("online"), DAY1, DAY1_CLOCK, false));

    let part = find_part(&h.raw(), "dataset=groups");
    let original = zstd::decode_all(&std::fs::read(&part).expect("read part")[..]).expect("unzstd");
    // Same length, different content: the length check must not be what
    // catches this, or the digest would be decoration.
    let mut tampered = original.clone();
    let last = tampered.len() - 3;
    tampered[last] = if tampered[last] == b'1' { b'2' } else { b'1' };
    assert_eq!(tampered.len(), original.len());
    std::fs::write(&part, zstd::encode_all(&tampered[..], 1).unwrap()).expect("write part");

    let target = h.db("from-corrupt");
    let out = h.derive(&target, DAY1, &[]);
    assert!(
        !out.status.success(),
        "a payload that does not match its manifest must stop the derive:\n{}",
        text(&out)
    );
    assert!(text(&out).contains("sha256"), "{}", text(&out));
}

/// A date nobody landed is a refusal that names the date — never a rebuild
/// from whatever else the lake happens to hold.
#[test]
fn a_date_that_was_never_landed_refuses_by_name() {
    let h = Harness::start();
    assert!(h.online(&h.db("online"), DAY1, DAY1_CLOCK, false));

    let out = h.derive(&h.db("never"), "2001-01-01", &[]);
    assert!(!out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains("no runs landed"), "{said}");
    assert!(said.contains("2001-01-01"), "{said}");
    assert!(said.contains("never falls back"), "{said}");
}

/// The fixture `tests/lake/derive.sh` reads, built by the real client through
/// the real landing zone.
///
/// Hand-writing zstd parts and a manifest in the shell would be easier and
/// would prove nothing — it would assert that the reader can read *the
/// shell's* idea of the layout. Change the key layout, the manifest fields or
/// the zstd level and the container gate downstream notices.
///
/// Run bare it builds into a temp dir and asserts what it built. Point
/// `DUMPSTAR_DERIVE_FIXTURE_OUT` at a directory and it writes there instead:
///
/// ```sh
/// DUMPSTAR_DERIVE_FIXTURE_OUT=/tmp/fixture \
///   cargo test -p dumpstar-lakehouse --test row_identical the_fixture
/// ```
///
/// It emits two days, the catalog each day's online refresh produced, and the
/// fixture upstream's origin. That last file is the awkward one and it is
/// honest: `raw/` is keyed by URL, so a replay has to build the same URLs the
/// fetch did. In production that is automatic — both sides are the compiled-in
/// constant — but a fixture upstream's port is ephemeral, so the container gate
/// has to be told what it was.
#[test]
fn the_fixture_the_container_gate_reads() {
    let out = std::env::var("DUMPSTAR_DERIVE_FIXTURE_OUT")
        .ok()
        .map(PathBuf::from);
    let keep = out.is_some();
    let h = Harness::at(out);

    h.day(1);
    assert!(h.online(&h.db("day1"), DAY1, DAY1_CLOCK, false), "day 1");
    std::fs::copy(h.db("day1"), h.db("online")).expect("carry day 1 forward");
    h.day(2);
    assert!(h.online(&h.db("online"), DAY2, DAY2_CLOCK, false), "day 2");
    std::fs::write(h.root.join("upstream-origin.txt"), h.upstream.base_url())
        .expect("record the fixture upstream's origin");

    // Both dates landed, each by one complete run.
    for date in [DAY1, DAY2] {
        assert!(
            h.raw()
                .join(format!(
                    "raw/source=tcgcsv/dataset=prices/ingest_date={date}"
                ))
                .exists(),
            "{date} landed no prices"
        );
    }

    if keep {
        println!("fixture written to {}", h.root.display());
    }
}

/// There is no default `--ingest-date`. Deriving an older day is the same
/// operation as deriving today's, and a job that could read the clock would
/// have two behaviours where it should have one.
#[test]
fn the_ingest_date_has_no_default() {
    let out = Command::new(env!("CARGO_BIN_EXE_dumpstar-lake-derive"))
        .args(["shared", "--db", "/nonexistent/shared.sqlite"])
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(text(&out).contains("--ingest-date"), "{}", text(&out));
}

// ---------------------------------------------------------------------------

/// The first `_manifest.json` under a prefix containing `needle`.
fn find_manifest(raw: &Path, needle: &str) -> PathBuf {
    find(raw, needle, "_manifest.json")
}

/// The first landed part under a prefix containing `needle`.
fn find_part(raw: &Path, needle: &str) -> PathBuf {
    find(raw, needle, ".zst")
}

fn find(dir: &Path, needle: &str, suffix: &str) -> PathBuf {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().contains(needle)
                && path.to_string_lossy().ends_with(suffix)
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no {suffix} under a prefix matching {needle} in {dir:?}"))
}

/// The nightly derive gives the catalog's WAL back before it exits (pd-t50h).
///
/// Stated against the SHIPPED BINARY over a real derivation, because the claim
/// is about the job production runs and not about the helper it calls. A source
/// grep for `checkpoint_truncate` would go green against a call in a branch
/// nothing takes.
///
/// **The held connection is the point of the test, not scaffolding.** SQLite
/// checkpoints and *deletes* the `-wal` when the LAST connection to a database
/// closes — so a run measured after the derive's process exits reports a
/// missing file, which reads exactly like "reclaimed" and is instead "nobody
/// was left holding it". That artifact produced a false finding the first time
/// this mechanism was measured (`deep-dives/attach-concurrency`, RESULT.md
/// finding B), and it is why the assertion below insists the file still EXISTS
/// as well as being empty. In production the connection this stands in for is
/// the server's.
#[test]
fn a_derive_leaves_no_wal_behind_on_the_catalog_it_built() {
    let h = Harness::start();
    let db = h.db("served");

    // A catalog to derive into. The nightly job runs against the one the box is
    // already serving, never a blank file.
    assert!(h.online(&db, DAY1, DAY1_CLOCK, false), "the first build");

    // The stand-in for `dumpstar serve`: a connection held open across the whole
    // derivation, exactly as a running server holds one.
    let served = rusqlite::Connection::open(&db).expect("hold the catalog open");
    let _: i64 = served
        .query_row("SELECT count(*) FROM cards", [], |r| r.get(0))
        .expect("read the catalog");

    h.day(2);
    let out = h.derive(&db, DAY1, &[]);
    assert!(
        out.status.success(),
        "the derive failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let wal = PathBuf::from(format!("{}-wal", db.display()));
    assert!(
        wal.exists(),
        "no -wal file at all, so this test measured nothing: the connection meant to keep \
         SQLite from deleting it on last close did not do its job"
    );
    let bytes = std::fs::metadata(&wal).expect("stat the -wal").len();
    assert_eq!(
        bytes, 0,
        "the derive exited leaving {bytes} bytes of WAL on the data volume. Nothing else \
         commits to this catalog, so nothing else will ever truncate it — it sits there \
         until tomorrow night. See pd-t50h."
    );

    drop(served);
}
