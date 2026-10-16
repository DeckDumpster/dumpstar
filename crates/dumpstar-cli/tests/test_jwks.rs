//! The Rust test Access issuer against the verifier the server runs
//! (dumpster-kit-tenancy's `AccessState`): its token is accepted, a token
//! signed by a key the JWKS does not publish is rejected.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use dumpster_kit_tenancy::access::{AccessConfig, AccessState};

const BIN: &str = env!("CARGO_BIN_EXE_dumpstar-test-jwks");

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn issue(dir: &std::path::Path, email: &str, forged: bool) -> String {
    let mut c = Command::new(BIN);
    c.arg("issue").arg(dir).arg(email);
    if forged {
        c.arg("--forged");
    }
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[tokio::test]
async fn issued_token_is_accepted_and_forged_one_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(BIN)
        .arg("serve")
        .arg(dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let _server = Server(child);
    let meta: serde_json::Value = serde_json::from_str(&first).unwrap();

    let access = AccessState::new(AccessConfig {
        team_domain: meta["issuer"].as_str().unwrap().into(),
        aud: meta["aud"].as_str().unwrap().into(),
        jwks_url: format!(
            "http://127.0.0.1:{}/cdn-cgi/access/certs",
            meta["port"].as_u64().unwrap()
        ),
    })
    .await
    .unwrap();

    let id = access
        .verify(&issue(dir.path(), "alice@example.com", false))
        .await
        .unwrap_or_else(|_| panic!("the issued token was rejected"));
    assert_eq!(id.email(), "alice@example.com");
    assert!(
        access
            .verify(&issue(dir.path(), "alice@example.com", true))
            .await
            .is_err(),
        "a token signed by an unpublished key was accepted"
    );
}
