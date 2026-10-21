//! The binary the skill runs: exit 0 and "ok" on a passing document, exit 1 and a FAIL line on a
//! failing one, exit 2 on input it cannot read.

use std::io::Write;
use std::process::{Command, Stdio};

use dumpstar_cube::fixture::{cube, pack};
use serde_json::json;

fn run(mode: &str, input: &str) -> (i32, String) {
    run_args(&[mode], input)
}

fn run_args(args: &[&str], input: &str) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn check_pack_passes_and_fails_by_exit_status() {
    let (code, out) = run("pack", &pack(1, "Heroism", "Command").to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let mut bad = pack(1, "Heroism", "Command");
    bad["cards"]["C1-3"]["cost"] = json!(1);
    let (code, out) = run("pack", &bad.to_string());
    assert_eq!(code, 1);
    assert!(out.contains("FAIL curve:"), "{out}");
}

#[test]
fn check_cube_passes_and_fails_by_exit_status() {
    let (code, out) = run("cube", &json!(cube()).to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let mut short = cube();
    short.pop();
    let (code, out) = run("cube", &json!(short).to_string());
    assert_eq!(code, 1);
    assert!(out.contains("FAIL cube-size:"), "{out}");
}

#[test]
fn input_that_is_not_json_exits_2() {
    assert_eq!(run("pack", "nope").0, 2);
}

#[test]
fn a_planned_document_without_reservations_is_not_failed_on_reserved() {
    let mut planned = pack(1, "Heroism", "Command");
    for l in planned["lines"].as_array_mut().unwrap() {
        l["reserved"] = json!(0);
    }
    let (code, out) = run("pack", &planned.to_string());
    assert_eq!(code, 1);
    assert!(out.contains("FAIL reserved:"), "{out}");
    let (code, out) = run_args(&["pack", "--planned"], &planned.to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let (code, out) = run_args(&["--planned", "cube"], &json!([planned]).to_string());
    assert!(!out.contains("FAIL reserved:"), "{out}");
    assert_eq!(code, 1, "cube-size still fails: {out}");
}
