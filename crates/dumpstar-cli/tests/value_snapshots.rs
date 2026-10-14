//! `dumpstar value-snapshots` against the shipped binary: the claim is about an
//! exit status and about each tenant's own file, so it is stated there.

use std::path::Path;
use std::process::Output;

fn dumpstar(home: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_dumpstar"))
        .args(args)
        .env("DUMPSTAR_HOME", home)
        .env("DUMPSTAR_USER", "collection")
        .output()
        .expect("the dumpstar binary did not run")
}

fn tenant_path(home: &Path, handle: &str) -> std::path::PathBuf {
    let out = dumpstar(home, &["tenant", "list"]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let line = text
        .lines()
        .find(|l| l.contains(handle))
        .unwrap_or_else(|| panic!("{handle} not listed: {text}"));
    let id = line
        .split_whitespace()
        .find(|w| w.len() == 26 && w.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| panic!("no database id in {line}"));
    home.join("tenants").join(format!("{id}.sqlite"))
}

fn setup(home: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    for h in ["alice", "bob"] {
        let out = dumpstar(home, &["tenant", "create", h]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let shared = home.join("shared.sqlite");
    let c = dumpstar_db::open_shared(&shared).unwrap();
    c.execute_batch("INSERT INTO sets (set_code, name) VALUES ('SOR','Spark of Rebellion');")
        .unwrap();
    (tenant_path(home, "alice"), tenant_path(home, "bob"))
}

#[test]
fn a_run_with_an_unreadable_tenant_exits_2_and_names_it() {
    let home = tempfile::tempdir().unwrap();
    let (alice, bob) = setup(home.path());
    std::fs::write(&bob, b"not a database, not a database, not a database").unwrap();

    let out = dumpstar(home.path(), &["value-snapshots", "--date", "2026-10-14"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bob: SKIPPED"), "{err}");
    assert!(!err.contains("alice: SKIPPED"), "{err}");
    let conn = dumpstar_db::open_user(&alice).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM collection_value_snapshot WHERE date='2026-10-14'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(n >= 1, "the readable tenant was not valued");
}

#[test]
fn two_readable_tenants_exit_0_and_both_get_a_series() {
    let home = tempfile::tempdir().unwrap();
    let (alice, bob) = setup(home.path());

    let out = dumpstar(home.path(), &["value-snapshots", "--date", "2026-10-14"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for p in [alice, bob] {
        let conn = dumpstar_db::open_user(&p).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_value_snapshot WHERE date='2026-10-14'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n >= 1, "{} has no value row for the date", p.display());
    }
}
