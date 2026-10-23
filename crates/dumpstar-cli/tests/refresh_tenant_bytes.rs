//! `dumpstar data refresh` against the shipped binary leaves every tenant
//! database and `shared.sqlite` byte-identical (pd-lunn; ci.sh item 9b).
//!
//! The refresh only LANDS: it fetches the upstreams and writes `raw/`, and the
//! catalog is built elsewhere. "Writes no database at all" is a claim about a
//! whole binary, so it is checked by running that binary over a data directory
//! that has two tenants and a catalog, and hashing every database before and
//! after. The upstream is the in-process fake the raw-coverage gate in
//! `src/data.rs` uses, so the run reaches its end without anybody's uptime.

use std::path::{Path, PathBuf};
use std::process::Output;

use dumpstar_ingest::test_upstream::{FakeUpstream, Reply};
use dumpstar_ingest::upstream::{ENV_OFFICIAL_BASE_URL, ENV_TCGCSV_BASE_URL};

const GROUPS: &str = r#"{"results":[
    {"groupId":23237,"name":"Spark of Rebellion","abbreviation":"SOR",
     "publishedOn":"2024-03-08"}],"success":true,"errors":[]}"#;
const EMPTY: &str = r#"{"results":[],"success":true,"errors":[]}"#;

fn route(target: &str, _n: usize) -> Reply {
    let path = target.split('?').next().unwrap_or(target);
    match path {
        "/api/cards" => Reply::ok(r#"{"data":[],"meta":{"pagination":{"pageCount":1}}}"#),
        "/last-updated.txt" => Reply::ok("2026-09-19T20:05:44+0000"),
        "/tcgplayer/79/groups" => Reply::ok(GROUPS),
        p if p.ends_with("/products") || p.ends_with("/prices") => Reply::ok(EMPTY),
        other => Reply::status(
            404,
            format!(
                r#"{{"error":"the refresh asked for {other}, which this fixture does not model"}}"#
            ),
        ),
    }
}

fn dumpstar(home: &Path, lake: &Path, upstream: &str, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_dumpstar"))
        .args(args)
        .env("DUMPSTAR_HOME", home)
        .env("DUMPSTAR_USER", "collection")
        .env("DUMPSTAR_LAKE_DIR", lake)
        .env(ENV_TCGCSV_BASE_URL, upstream)
        .env(ENV_OFFICIAL_BASE_URL, upstream)
        // A lake.env on the machine running this must never be read.
        .env("HOME", home)
        .env_remove("DUMPSTAR_LAKE_ENV")
        .output()
        .expect("the dumpstar binary did not run")
}

/// Every database under `home`, as (relative path, contents).
fn databases(home: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "sqlite") {
                let rel = path.strip_prefix(home).unwrap().to_path_buf();
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_refresh_writes_no_tenant_database_and_no_catalog() {
    let home = tempfile::tempdir().unwrap();
    let lake = tempfile::tempdir().unwrap();
    let upstream = FakeUpstream::start(route);
    let base = upstream.base_url();

    for handle in ["alice", "bob"] {
        let out = dumpstar(
            home.path(),
            lake.path(),
            &base,
            &["tenant", "create", handle],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let shared = home.path().join("shared.sqlite");
    dumpstar_db::open_shared(&shared)
        .unwrap()
        .execute_batch("INSERT INTO sets (set_code, name) VALUES ('SOR','Spark of Rebellion');")
        .unwrap();

    let before = databases(home.path());
    assert!(
        before.len() >= 3,
        "the fixture should hold two tenants and the catalog: {:?}",
        before.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );

    let out = dumpstar(home.path(), lake.path(), &base, &["data", "refresh"]);
    assert!(
        out.status.success(),
        "the refresh did not run to the end, so this gate would prove nothing:\n{}\n{}\nrequests: {:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        upstream.requests()
    );
    assert!(
        !upstream.requests().is_empty(),
        "the refresh fetched nothing from the fixture upstream"
    );

    let after = databases(home.path());
    assert_eq!(
        before.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        after.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        "the refresh created or removed a database"
    );
    for ((path, was), (_, now)) in before.iter().zip(&after) {
        assert!(was == now, "the refresh changed {}", path.display());
    }
}
