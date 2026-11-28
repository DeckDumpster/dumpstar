//! What `deploy/ci.sh` does with the deny list: its own functions, run by bash with `cargo`
//! replaced by a stub that records each call's arguments.

#[macro_use]
extern crate spira_test_id;

use std::path::Path;
use std::process::Command;

/// The text of `name() { … }` in ci.sh, up to the first `}` at the margin.
fn function(ci: &str, name: &str) -> String {
    let start = ci
        .find(&format!("\n{name}() {{"))
        .unwrap_or_else(|| panic!("deploy/ci.sh defines no {name}"));
    let len = ci[start..].find("\n}\n").expect("the function closes") + 3;
    ci[start..start + len].to_string()
}

/// `cargo run` is the deny-list tool: it prints $ANSWER and exits $TOOL_RC. A `--list` prints
/// enough tests to satisfy the isolation gate's count.
const STUBS: &str = r#"
set -euo pipefail
tier() { true; }
step() { :; }
dumpstar_rust_cache_report() { :; }
cargo() {
    { printf '[%s]' "$@"; echo; } >> "$CALLS"
    case "$1 $*" in
        run*) printf '%s' "$ANSWER"; return "$TOOL_RC" ;;
        *--list*) for i in 1 2 3 4 5 6 7 8 9; do echo "t$i: test"; done ;;
    esac
}
"#;

struct Ran {
    status: Option<i32>,
    stderr: String,
    calls: Vec<String>,
}

/// Run ci.sh's `gate` with a deny-list tool that answers `answer` and exits `tool_rc`.
fn run(gate: &str, list_is_set: bool, answer: &str, tool_rc: u8) -> Ran {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ci = std::fs::read_to_string(repo.join("deploy/ci.sh")).unwrap();
    let functions: String = ["require_deny_list", "deny_args", "g_nextest", "g_isolation"]
        .map(|name| function(&ci, name))
        .concat();
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("list");
    std::fs::write(&list, "").unwrap();
    let calls = dir.path().join("calls");
    let mut bash = Command::new("bash");
    bash.arg("-c")
        .arg(format!("{STUBS}\n{functions}\n{gate}\necho finished >&2"))
        .env("REPO_DIR", dir.path())
        .env("CALLS", &calls)
        .env("ANSWER", answer)
        .env("TOOL_RC", tool_rc.to_string())
        .env_remove("SPIRA_TEST_DENY_LIST");
    if list_is_set {
        bash.env("SPIRA_TEST_DENY_LIST", &list);
    }
    let out = bash.output().unwrap();
    Ran {
        status: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        calls: std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect(),
    }
}

const GATES: [&str; 2] = ["g_nextest", "g_isolation"];

#[test_id(t8vod5k)]
#[test]
fn a_rust_gate_with_no_deny_list_names_the_key_before_any_cargo_call() {
    for gate in GATES {
        let ran = run(gate, false, "", 0);
        assert_eq!(ran.status, Some(1), "{gate}: {}", ran.stderr);
        assert!(
            ran.stderr.contains("SPIRA_TEST_DENY_LIST"),
            "{}",
            ran.stderr
        );
        assert!(ran.stderr.contains("test-registry deny-list --at"));
        assert!(ran.calls.is_empty(), "{gate}: {:?}", ran.calls);
    }
}

#[test_id(t2cssvj)]
#[test]
fn a_refused_deny_list_stops_the_gate_before_any_test_runs() {
    for gate in GATES {
        let ran = run(gate, true, "", 1);
        assert_eq!(ran.status, Some(1), "{gate}: {}", ran.stderr);
        let ran_tests: Vec<_> = ran
            .calls
            .iter()
            .filter(|c| c.starts_with("[nextest][run]") || c.starts_with("[test]"))
            .collect();
        assert!(ran_tests.is_empty(), "{gate}: {ran_tests:?}");
    }
}

#[test_id(tdt8maz)]
#[test]
fn the_nextest_run_gets_the_filter_as_two_arguments() {
    let ran = run("g_nextest", true, "-E\nnot (test(/::t0aaaaa$/))\n", 0);
    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    let want =
        "[nextest][run][--profile][ci][--workspace][--no-fail-fast][-E][not (test(/::t0aaaaa$/))]";
    assert!(ran.calls.iter().any(|c| c == want), "{:?}", ran.calls);
}

#[test_id(t352x0e)]
#[test]
fn a_list_that_denies_nothing_adds_no_argument_to_the_nextest_run() {
    let ran = run("g_nextest", true, "", 0);
    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    let want = "[nextest][run][--profile][ci][--workspace][--no-fail-fast]";
    assert!(ran.calls.iter().any(|c| c == want), "{:?}", ran.calls);
}

#[test_id(tp2o1aq)]
#[test]
fn the_isolation_count_and_run_both_skip_the_denied_tests() {
    let ran = run("g_isolation", true, "--skip\n::t0aaaaa\n", 0);
    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    let base = "[test][-p][dumpstar-server][--lib][--][tests::isolation::][--skip][::t0aaaaa]";
    let tests: Vec<_> = ran
        .calls
        .iter()
        .filter(|c| c.starts_with("[test]"))
        .collect();
    assert_eq!(tests, [&format!("{base}[--list]"), base]);
}
