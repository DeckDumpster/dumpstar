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
fn a_leader_in_two_packs_fails_the_cube_and_names_both() {
    let mut c = cube();
    let dup = c[0]["cards"]["L1"].clone();
    c[1]["cards"]["L2"] = dup;
    let (code, out) = run("cube", &json!(c).to_string());
    assert_eq!(code, 1);
    assert!(
        out.contains("FAIL cube-singleton:") && out.contains("Pack 1, Pack 2"),
        "{out}"
    );
}

#[test]
fn two_packs_sharing_a_single_owned_card_fail_the_cube_and_name_both() {
    let mut c = cube();
    c[0]["cards"]["C1-0"]["owned_count"] = json!(1);
    // Pack 2 takes the same card id as pack 1's C1-0, in place of its own C2-0.
    c[1]["cards"]["C1-0"] = c[0]["cards"]["C1-0"].clone();
    c[1]["lines"][3]["card_id"] = json!("C1-0");
    let (code, out) = run("cube", &json!(c).to_string());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("FAIL cube-copies:")
            && out.contains("'C1-0'")
            && out.contains("Pack 1, Pack 2"),
        "{out}"
    );
}

#[test]
fn cube_takes_one_file_per_pack_and_a_lone_pack_passes_pack_mode() {
    let dir = std::env::temp_dir().join(format!("swu-cube-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<String> = cube()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let f = dir.join(format!("pack{i}.json"));
            std::fs::write(&f, p.to_string()).unwrap();
            f.to_str().unwrap().to_string()
        })
        .collect();
    let mut args = vec!["cube"];
    args.extend(files.iter().map(String::as_str));
    let (code, out) = run_args(&args, "");
    assert_eq!((code, out.trim()), (0, "ok"));
    let (code, out) = run_args(&["pack", &files[0]], "");
    assert_eq!((code, out.trim()), (0, "ok"));
    let _ = std::fs::remove_dir_all(&dir);
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

#[test]
fn a_digest_that_does_not_match_the_lines_is_refused_before_any_rule() {
    let mut saved = pack(1, "Heroism", "Command");
    saved["lines_digest"] =
        json!(dumpstar_cube::lines_digest(&saved["lines"], &saved["cards"]).unwrap());
    let (code, out) = run("pack", &saved.to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let mut altered = saved.clone();
    altered["lines"][0]["qty"] = json!(2);
    let (code, out) = run("pack", &altered.to_string());
    assert_eq!((code, out.trim()), (2, ""));
    altered = saved.clone();
    altered["cards"]["L1"]["game"]["aspects"] = json!(["Heroism", "Cunning"]);
    let (code, out) = run("pack", &altered.to_string());
    assert_eq!((code, out.trim()), (2, ""));
}

#[test]
fn allocate_reserves_shared_cards_across_packs_and_names_a_shortfall() {
    let mut doc = json!({
        "owned": {"Longbeam Cruiser": 1, "Command Unit": 1},
        "packs": [
            {"name": "Pack 1", "slots": 1, "eligible": ["Longbeam Cruiser", "Command Unit"]},
            {"name": "Pack 2", "slots": 1, "eligible": ["Longbeam Cruiser"]},
        ]
    });
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("reserve Longbeam Cruiser: Pack 2"), "{out}");
    doc["owned"]["Command Unit"] = json!(0);
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 1);
    assert!(
        out.contains("FAIL allocation:") && out.contains("Longbeam Cruiser"),
        "{out}"
    );
}
