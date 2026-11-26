//! The shared Rust compile cache wiring (sp-2i16qf, sp-ma66i8, sp-p7xf0n):
//! hermetic, no cargo, no podman, no network.
//!
//! The behaviour tests spawn `bash` and `sh`, the exception to
//! law-a-unit-test-calls-no-binary: the unit under test is a shell script
//! (`deploy/rust-cache.sh`, `deploy/sccache-probe.sh`, `deploy/runner-deps.sh`),
//! so the test has to run it. `curl`, `sccache` and `rustc` are stubs where a case
//! needs a particular answer from them.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};

use flate2::Compression;
use flate2::write::GzEncoder;
use tempfile::TempDir;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    fs::read_to_string(repo().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A file without its whole-line comments, so a commented-out use is no use.
fn code(path: &str) -> String {
    read(path)
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A throwaway `HOME`, sccache directory and stub directory for one case.
struct Sandbox {
    dir: TempDir,
    // Under `cargo test` the cases share a process, and a child forked while a
    // stub is open for writing makes running that stub fail with ETXTBSY.
    _serial: MutexGuard<'static, ()>,
}

impl Sandbox {
    fn new() -> Self {
        static SERIAL: Mutex<()> = Mutex::new(());
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        Sandbox {
            dir: tempfile::tempdir().unwrap(),
            _serial: serial,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn stub(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A command in the repo root that sees nothing of the caller's cache
    /// settings: the rust tier runs this test with its own cache switched on.
    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(repo());
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name == "RUSTC_WRAPPER"
                || name == "CARGO_INCREMENTAL"
                || name.starts_with("SCCACHE_")
                || name.starts_with("DUMPSTAR_")
            {
                cmd.env_remove(&key);
            }
        }
        cmd.env("HOME", self.path("home"))
            .env("DUMPSTAR_SCCACHE_DIR", self.path("sccache-dir"));
        cmd
    }

    /// Source `deploy/rust-cache.sh`, then run `call`.
    fn rust_cache(&self, call: &str, env: &[(&str, &str)]) -> String {
        let out = self
            .command("bash")
            .args(["-c", &format!(". deploy/rust-cache.sh; {call}")])
            .envs(env.iter().copied())
            .output()
            .expect("bash runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The probe wraps `rustc -vV`. A stub answers it: the rustup proxy, under
    /// this case's empty `HOME`, would download a toolchain.
    fn probe(&self, sccache: &Path) -> Output {
        self.stub("bin/rustc", "#!/bin/sh\n");
        self.command("sh")
            .arg("deploy/sccache-probe.sh")
            .arg(sccache)
            .env("PATH", self.stub_path())
            .output()
            .expect("sh runs")
    }

    /// The system tools, behind this case's stubs.
    fn stub_path(&self) -> String {
        format!("{}:/usr/bin:/bin", self.path("bin").display())
    }

    /// `runner-deps.sh --sccache` with `bin/curl` as its only way to download.
    fn install_sccache(&self) -> (Output, String) {
        let out = self
            .command("bash")
            .args(["deploy/runner-deps.sh", "--sccache"])
            .env("PATH", self.stub_path())
            .output()
            .expect("bash runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out, text)
    }
}

#[test]
fn disabled_cache_says_so() {
    let out = Sandbox::new().rust_cache(
        "dumpstar_rust_cache_enable",
        &[("DUMPSTAR_RUST_CACHE", "off")],
    );
    assert!(out.contains("disabled (DUMPSTAR_RUST_CACHE=off)"), "{out}");
    assert!(out.contains("compiling cold"), "{out}");
}

#[test]
fn missing_sccache_says_so() {
    let out = Sandbox::new().rust_cache("dumpstar_rust_cache_enable", &[("PATH", "/usr/bin:/bin")]);
    assert!(out.contains("not installed"), "{out}");
    assert!(out.contains("compiling cold"), "{out}");
}

#[test]
fn unreachable_store_falls_back_says_so() {
    let sandbox = Sandbox::new();
    // An installed sccache, so the case reaches the store on every machine.
    sandbox.stub("sccache-dir/sccache", "#!/bin/sh\n");
    let out = sandbox.rust_cache(
        "dumpstar_rust_cache_enable",
        &[("DUMPSTAR_SCCACHE_ENDPOINT", "http://127.0.0.1:1")],
    );
    assert!(out.contains("unreachable"), "{out}");
    assert!(out.contains("compiling cold"), "{out}");
}

#[test]
fn report_states_a_cold_run() {
    let out = Sandbox::new().rust_cache(
        "DUMPSTAR_RUST_CACHE_STATE=off dumpstar_rust_cache_report",
        &[],
    );
    assert!(out.contains("not in use"), "{out}");
}

#[test]
fn ci_sh_and_image_lib_sh_use_the_cache() {
    assert!(code("deploy/ci.sh").contains("dumpstar_rust_cache_enable"));
    assert!(code("deploy/image-lib.sh").contains("dumpstar_rust_cache_buildargs"));
}

#[test]
fn containerfile_builder_consumes_the_cache() {
    let containerfile = code("Containerfile");
    assert!(containerfile.contains("RUSTC_WRAPPER"));
    assert!(containerfile.contains("sccache-bin"));
}

#[test]
fn dead_sccache_server_fails_the_probe_says_why() {
    let sandbox = Sandbox::new();
    let sccache = sandbox.stub(
        "sccache",
        "#!/bin/sh\necho \"sccache: error: Timed out waiting for server startup.\" >&2\nexit 2\n",
    );
    let out = sandbox.probe(&sccache);
    assert!(!out.status.success(), "probe passed a dead server: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("did not start"), "{out:?}");
}

#[test]
fn working_sccache_passes_the_probe() {
    let sandbox = Sandbox::new();
    let sccache = sandbox.stub("sccache", "#!/bin/sh\nexec \"$@\"\n");
    let out = sandbox.probe(&sccache);
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn containerfile_probes_before_enabling_the_wrapper() {
    assert!(code("Containerfile").contains("sccache-probe.sh"));
}

#[test]
fn sccache_flag_a_failed_download_is_a_note_nothing_else_is_installed() {
    let sandbox = Sandbox::new();
    sandbox.stub("bin/curl", "#!/bin/sh\nexit 22\n");
    let (out, text) = sandbox.install_sccache();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("could not install sccache"), "{text}");
    assert!(!text.contains("apt"), "{text}");
}

#[test]
fn sccache_flag_installs_the_releases_binary() {
    let sandbox = Sandbox::new();
    let tarball = sandbox.path("release.tgz");
    let gz = GzEncoder::new(fs::File::create(&tarball).unwrap(), Compression::fast());
    let mut archive = tar::Builder::new(gz);
    let binary = b"#!/bin/sh\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(binary.len() as u64);
    header.set_mode(0o755);
    archive
        .append_data(
            &mut header,
            "sccache-v0.10.0-x86_64-unknown-linux-musl/sccache",
            &binary[..],
        )
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();
    sandbox.stub(
        "bin/curl",
        &format!(
            r#"#!/bin/sh
while [ $# -gt 0 ]; do
    [ "$1" = -o ] && exec cp "{}" "$2"
    shift
done
exit 2
"#,
            tarball.display()
        ),
    );

    let (out, text) = sandbox.install_sccache();
    assert_eq!(out.status.code(), Some(0), "{text}");
    let installed = fs::metadata(sandbox.path("sccache-dir/sccache"))
        .unwrap_or_else(|e| panic!("sccache not installed: {e}\n{text}"));
    assert_ne!(installed.permissions().mode() & 0o111, 0, "{text}");
}

/// The `key = ` line of one `[tier.<name>]` section of `.spira/testenv.toml`.
fn testenv_tier_line(tier: &str, key: &str) -> String {
    let header = format!("[tier.{tier}]");
    let prefix = format!("{key} = ");
    read(".spira/testenv.toml")
        .lines()
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("{header} has no {key} line"))
        .to_owned()
}

#[test]
fn testenv_rust_tier_fetches_sccache_before_ci_sh() {
    let argv = testenv_tier_line("rust", "argv");
    let (_, after_fetch) = argv
        .split_once("runner-deps.sh --sccache && ")
        .unwrap_or_else(|| panic!("rust tier does not fetch sccache: {argv}"));
    assert!(after_fetch.contains("deploy/ci.sh"), "{argv}");
}

#[test]
fn testenv_isolation_tier_runs_in_the_rust_image() {
    // Without the group list, ci.sh's dependency check demands podman, node and
    // chromium, which the image lacks, and no isolation test runs.
    let env = testenv_tier_line("isolation", "env");
    assert!(
        env.contains(r#"DUMPSTAR_RUNNER_DEPS_ONLY = "base rust""#),
        "isolation tier checks every runner-deps group: {env}"
    );
    let argv = testenv_tier_line("isolation", "argv");
    for step in ["apt-get install", "runner-deps.sh --sccache && "] {
        let (_, after_step) = argv
            .split_once(step)
            .unwrap_or_else(|| panic!("isolation tier lacks `{step}`: {argv}"));
        assert!(after_step.contains("deploy/ci.sh"), "{argv}");
    }
}
