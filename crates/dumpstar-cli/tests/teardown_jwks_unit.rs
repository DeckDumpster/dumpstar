//! `deploy/teardown.sh` stops the transient JWKS server unit that
//! `deploy/setup.sh <instance> --test` recorded for the instance. The code under
//! test is a shell script, so this spawns bash: a justified exception to
//! law-a-unit-test-calls-no-binary. `systemctl` and `podman` are stubs that log
//! their argv, so no real unit or container store is touched.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

const UNIT: &str = "dumpstar-test-jwks-t1-1-2";

struct Teardown {
    dir: TempDir,
}

impl Teardown {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["home", "run", "bin"] {
            std::fs::create_dir(dir.path().join(sub)).unwrap();
        }
        for tool in ["systemctl", "podman"] {
            let stub = dir.path().join("bin").join(tool);
            std::fs::write(&stub, "#!/bin/sh\necho \"$*\" >>\"$STUB_LOG\"\n").unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    fn conf_dir(&self) -> PathBuf {
        self.dir.path().join("home/.config/dumpstar/t1")
    }

    fn plant_record(&self) {
        std::fs::create_dir_all(self.conf_dir()).unwrap();
        std::fs::write(
            self.conf_dir().join("test-jwks.unit"),
            format!("{UNIT}\n"),
        )
        .unwrap();
    }

    /// Runs `teardown.sh t1 <args>`, requires success and returns the stub log.
    fn run(&self, args: &[&str]) -> String {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/teardown.sh");
        let log = self.dir.path().join("stub.log");
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let out = Command::new("bash")
            .arg(script)
            .arg("t1")
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.dir.path().join("home"))
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            // Set but empty: no store.env is read and no alternate store is used.
            .env("DUMPSTAR_STORE_ROOT", "")
            .env("STUB_LOG", &log)
            .output()
            .expect("bash runs");
        assert!(out.status.success(), "teardown failed: {out:?}");
        let log = std::fs::read_to_string(log).unwrap();
        assert!(
            log.contains("--user stop dumpstar-t1\n"),
            "the stub saw none of teardown's calls:\n{log}"
        );
        log
    }
}

#[test]
fn teardown_stops_the_recorded_jwks_unit() {
    let t = Teardown::new();
    t.plant_record();
    let log = t.run(&[]);
    assert!(
        log.contains(&format!("--user stop {UNIT}\n")),
        "no stop of {UNIT}:\n{log}"
    );
}

#[test]
fn purge_stops_the_recorded_jwks_unit_before_deleting_its_record() {
    let t = Teardown::new();
    t.plant_record();
    let log = t.run(&["--purge"]);
    assert!(
        log.contains(&format!("--user stop {UNIT}\n")),
        "no stop of {UNIT}:\n{log}"
    );
    assert!(!t.conf_dir().exists(), "--purge left the conf dir");
}

#[test]
fn teardown_without_a_record_stops_no_jwks_unit() {
    let t = Teardown::new();
    let log = t.run(&[]);
    assert!(!log.contains("dumpstar-test-jwks"), "{log}");
}
