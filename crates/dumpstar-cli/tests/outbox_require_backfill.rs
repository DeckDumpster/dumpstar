//! `dumpstar outbox status --require-backfill` (pd-0h2p).
//!
//! The precondition `deploy/setup-lake.sh --arm-shipper` arms the nightly
//! shipment on. It is stated here against the shipped binary rather than
//! against `outbox::last_backfill` directly, because the claim is about an
//! **exit status**: the bash that arms the timer reads nothing but that, and
//! a report that says "never emitted" in its text while exiting 0 is exactly
//! the shape of check that arms a box early (`pd-whsw`) — and the shape of
//! listing `pd-cxq4` taught this repo never to conclude anything from.

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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn a_collection_that_was_never_backfilled_refuses_to_be_called_ready() {
    let home = tempfile::tempdir().unwrap();

    // The report itself is unchanged and still exits 0 — it is a report, and
    // `dumpstar outbox status` is run by hand far more often than by a script.
    let report = dumpstar(home.path(), &["outbox", "status"]);
    assert!(
        report.status.success(),
        "the plain report is not a check: {}",
        stderr(&report)
    );

    // The flag is what makes it one.
    let checked = dumpstar(home.path(), &["outbox", "status", "--require-backfill"]);
    assert!(
        !checked.status.success(),
        "an un-backfilled collection was reported ready to ship"
    );
    assert!(
        stderr(&checked).contains("NOT ready"),
        "the refusal does not say what it refused: {}",
        stderr(&checked)
    );
}

#[test]
fn a_backfilled_collection_is_ready() {
    let home = tempfile::tempdir().unwrap();

    let emit = dumpstar(home.path(), &["outbox", "emit", "--all"]);
    assert!(
        emit.status.success(),
        "the backfill failed: {}",
        stderr(&emit)
    );

    let checked = dumpstar(home.path(), &["outbox", "status", "--require-backfill"]);
    assert!(
        checked.status.success(),
        "a backfilled collection was refused: {}",
        stderr(&checked)
    );
}

/// A ledger that holds only a redrive is not a collection that has been
/// described to the zone in full. `runs.is_empty()` cannot tell the two
/// apart, which is why the check asks for a *backfill* specifically.
#[test]
fn a_redrive_is_not_a_backfill() {
    let home = tempfile::tempdir().unwrap();

    let redrive = dumpstar(home.path(), &["outbox", "emit", "--seq", "1..1"]);
    assert!(
        redrive.status.success(),
        "the redrive failed: {}",
        stderr(&redrive)
    );

    let checked = dumpstar(home.path(), &["outbox", "status", "--require-backfill"]);
    assert!(
        !checked.status.success(),
        "a redrive was accepted as a backfill"
    );
}
