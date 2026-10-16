//! `tests/lib/ports.sh`'s `free_port` never hands back a port something is
//! already listening on. The live listener is a std `TcpListener` held by this
//! test, so no Python (or nc) is needed; `tests/lib/ports_test.sh` §4 held the
//! same fixture before 2026-10-15.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

fn ports_sh() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/lib/ports.sh")
}

fn bash(script: &str) -> std::process::Output {
    Command::new("bash")
        .args(["-c", script, "bash"])
        .arg(ports_sh())
        .output()
        .expect("bash runs")
}

#[test]
fn port_in_use_sees_a_live_listener() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let out = bash(&format!(". \"$1\"; port_in_use {port}"));
    assert!(
        out.status.success(),
        "port_in_use missed listener on {port}"
    );
}

#[test]
fn free_port_never_returns_a_listening_port() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    let out = bash(". \"$1\"; for _ in $(seq 30); do free_port || exit 1; done");
    assert!(out.status.success(), "free_port failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let ports: Vec<&str> = stdout.lines().collect();
    assert_eq!(ports.len(), 30);
    assert!(!ports.contains(&port.as_str()), "returned held port {port}");
}
