//! The shared Rust compile cache wiring (sp-2i16qf, sp-ma66i8, sp-p7xf0n,
//! sp-p7xf0n.1): hermetic, no cargo, no podman, no network.
//!
//! The behaviour tests spawn `bash` and `sh`, the exception to
//! law-a-unit-test-calls-no-binary: the unit under test is a shell script
//! (`deploy/rust-cache.sh`, `deploy/sccache-probe.sh`, `deploy/runner-deps.sh`,
//! `deploy/image-lib.sh`), so the test has to run it. `curl`, `nix`, `nix-store`,
//! `podman`, `id`, `sccache` and `rustc` are stubs where a case needs a particular
//! answer from them.

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};

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
        // The machine's own daemon-installed Nix, if it has one, is out of
        // sight: a case that has one puts it under this directory.
        cmd.env("HOME", self.path("home"))
            .env("DUMPSTAR_SCCACHE_DIR", self.path("sccache-dir"))
            .env("DUMPSTAR_NIX_SYSTEM_PROFILE", self.path("nix-system"));
        cmd
    }

    /// Source `deploy/rust-cache.sh`, then run `call`.
    fn rust_cache(&self, call: &str, env: &[(&str, &str)]) -> String {
        self.run_rust_cache(call, env).1
    }

    /// As `rust_cache`, with the exit status of `call`.
    fn run_rust_cache(&self, call: &str, env: &[(&str, &str)]) -> (Option<i32>, String) {
        let out = self
            .command("bash")
            .args(["-c", &format!(". deploy/rust-cache.sh; {call}")])
            .envs(env.iter().copied())
            .output()
            .expect("bash runs");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        (out.status.code(), text)
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

    /// `runner-deps.sh` seeing only this case's stubs and the system tools.
    fn runner_deps(&self, mode: &str, env: &[(&str, &str)]) -> (Output, String) {
        let out = self
            .command("bash")
            .args(["deploy/runner-deps.sh", mode])
            .env("PATH", self.stub_path())
            .envs(env.iter().copied())
            .output()
            .expect("bash runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out, text)
    }

    /// `bin/curl` appending every call to `curl.log`, then running `body`.
    fn recording_curl(&self, body: &str) {
        let log = self.path("curl.log");
        self.stub(
            "bin/curl",
            &format!("#!/bin/sh\necho \"$*\" >>'{}'\n{body}\n", log.display()),
        );
    }

    /// A `curl` that serves the Nix installer and its published checksum.
    fn nix_release(&self, installer: &Path, checksum: &Path) {
        self.recording_curl(&format!(
            r#"while [ $# -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        http*) url=$1 ;;
    esac
    shift
done
case "$url" in
    */install.sha256) cp '{}' "$out" ;;
    */install) cp '{}' "$out" ;;
    *) exit 22 ;;
esac"#,
            checksum.display(),
            installer.display()
        ));
    }

    /// What was logged to `name`, empty when nothing was.
    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    /// A fake two-path store closure, read-only as the real store is, behind an
    /// out-link at `sccache-dir`, with a `nix-store` that lists it. Returns the
    /// sccache store path.
    fn fake_closure(&self) -> PathBuf {
        self.stub("fake/aaa-sccache/bin/sccache", "#!/bin/sh\n");
        self.stub("fake/bbb-libc/lib/libc.so", "");
        let root = fs::canonicalize(self.path("fake")).unwrap();
        let (sccache, libc) = (root.join("aaa-sccache"), root.join("bbb-libc"));
        self.chmod_fake("a-w");
        symlink(&sccache, self.path("sccache-dir")).unwrap();
        self.stub(
            "bin/nix-store",
            &format!(
                "#!/bin/sh\n[ \"$*\" = '--query --requisites {0}' ] || exit 1\n\
                 printf '%s\\n' '{0}' '{1}'\n",
                sccache.display(),
                libc.display()
            ),
        );
        sccache
    }

    fn chmod_fake(&self, mode: &str) {
        let status = Command::new("/bin/chmod")
            .args(["-R", mode])
            .arg(self.path("fake"))
            .status()
            .expect("chmod runs");
        assert!(status.success());
    }
}

/// A `nix` whose `build` leaves `bin/sccache` at the out-link.
const WORKING_NIX: &str = r#"#!/bin/sh
echo "$*" >>"$(dirname "$HOME")/nix.log"
ls -A >"$(dirname "$HOME")/nix.cwd"
while [ $# -gt 0 ]; do
    if [ "$1" = --out-link ]; then
        mkdir -p "$2/bin" && printf '#!/bin/sh\n' >"$2/bin/sccache" && chmod +x "$2/bin/sccache"
        exit
    fi
    shift
done
exit 2
"#;

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
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
    sandbox.stub("sccache-dir/bin/sccache", "#!/bin/sh\n");
    let (status, out) = sandbox.run_rust_cache(
        "dumpstar_rust_cache_enable",
        &[
            ("DUMPSTAR_RUST_CACHE", "on"),
            ("DUMPSTAR_SCCACHE_ENDPOINT", "http://127.0.0.1:1"),
        ],
    );
    assert_eq!(status, Some(0), "{out}");
    assert!(out.contains("unreachable"), "{out}");
    assert!(out.contains("compiling cold"), "{out}");
}

#[test]
fn required_cache_unreachable_store_fails_and_names_the_endpoint() {
    let sandbox = Sandbox::new();
    sandbox.stub("sccache-dir/bin/sccache", "#!/bin/sh\n");
    let (status, out) = sandbox.run_rust_cache(
        "dumpstar_rust_cache_enable",
        &[
            ("DUMPSTAR_RUST_CACHE", "required"),
            ("DUMPSTAR_SCCACHE_ENDPOINT", "http://127.0.0.1:1"),
        ],
    );
    assert_eq!(status, Some(1), "{out}");
    assert!(out.contains("http://127.0.0.1:1 unreachable"), "{out}");
}

#[test]
fn required_cache_ignores_a_sccache_on_path() {
    let sandbox = Sandbox::new();
    sandbox.stub("bin/sccache", "#!/bin/sh\n");
    sandbox.stub("bin/curl", "#!/bin/sh\n");
    let (status, out) = sandbox.run_rust_cache(
        "dumpstar_rust_cache_enable",
        &[
            ("DUMPSTAR_RUST_CACHE", "required"),
            ("PATH", &sandbox.stub_path()),
        ],
    );
    assert_eq!(status, Some(1), "{out}");
    assert!(out.contains("not installed"), "{out}");
}

#[test]
fn image_build_context_carries_the_store_closure() {
    let sandbox = Sandbox::new();
    let sccache = sandbox.fake_closure();
    let tmp = sandbox.path("ctx-tmp");
    fs::create_dir(&tmp).unwrap();
    // Sourcing `rust-cache.sh` resets the state, so the call sets it.
    let (status, out) = sandbox.run_rust_cache(
        "DUMPSTAR_RUST_CACHE_STATE=on dumpstar_rust_cache_buildargs",
        &[
            ("SCCACHE_WEBDAV_ENDPOINT", "http://store.invalid"),
            ("TMPDIR", tmp.to_str().unwrap()),
            ("PATH", &sandbox.stub_path()),
        ],
    );
    sandbox.chmod_fake("u+w");
    assert_eq!(status, Some(0), "{out}");
    let context = out
        .lines()
        .find_map(|line| line.strip_prefix("sccache-bin="))
        .unwrap_or_else(|| panic!("no sccache-bin context: {out}"));
    let store = Path::new(context).join("store");
    assert!(
        is_executable(&store.join("aaa-sccache/bin/sccache")),
        "{out}"
    );
    assert!(store.join("bbb-libc/lib/libc.so").is_file(), "{out}");
    let build_arg = format!("SCCACHE_BIN={}/bin/sccache", sccache.display());
    assert!(out.lines().any(|line| line == build_arg), "{out}");
}

#[test]
fn image_build_context_is_removed_whether_the_build_passes_or_fails() {
    let sandbox = Sandbox::new();
    sandbox.fake_closure();
    // Exits 9 unless the context it is handed holds the closure.
    sandbox.stub(
        "bin/podman",
        r#"#!/bin/sh
[ "$1" = build ] || exit 0
for a; do
    case "$a" in sccache-bin=*) [ -d "${a#sccache-bin=}/store" ] && exit "$STUB_BUILD_RC" ;; esac
done
exit 9
"#,
    );
    let tmp = sandbox.path("ctx-tmp");
    for build_rc in [0, 1] {
        fs::create_dir_all(&tmp).unwrap();
        let (status, out) = sandbox.run_rust_cache(
            ". deploy/image-lib.sh; DUMPSTAR_RUST_CACHE_STATE=on; \
             dumpstar_image_ensure probe:x \"$PWD\" 2>&1",
            &[
                ("SCCACHE_WEBDAV_ENDPOINT", "http://store.invalid"),
                ("TMPDIR", tmp.to_str().unwrap()),
                ("PATH", &sandbox.stub_path()),
                ("STUB_BUILD_RC", &build_rc.to_string()),
            ],
        );
        assert_eq!(status, Some(build_rc), "{out}");
        let left: Vec<_> = fs::read_dir(&tmp).unwrap().collect();
        assert!(left.is_empty(), "build exiting {build_rc} left {left:?}");
    }
    sandbox.chmod_fake("u+w");
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
fn an_image_whose_closure_cannot_be_listed_is_not_built_cold_when_required() {
    let sandbox = Sandbox::new();
    sandbox.stub("bin/nix-store", "#!/bin/sh\nexit 1\n");
    let built = sandbox.path("built");
    sandbox.stub(
        "bin/podman",
        &format!(
            "#!/bin/sh\n[ \"$1\" = build ] && echo \"$*\" >'{}'\nexit 0\n",
            built.display()
        ),
    );
    let build = |mode: &str| {
        sandbox.run_rust_cache(
            ". deploy/image-lib.sh; DUMPSTAR_RUST_CACHE_STATE=on; \
             dumpstar_image_ensure probe:x \"$PWD\" 2>&1",
            &[
                ("DUMPSTAR_RUST_CACHE", mode),
                ("SCCACHE_WEBDAV_ENDPOINT", "http://store.invalid"),
                ("PATH", &sandbox.stub_path()),
            ],
        )
    };
    let (status, out) = build("required");
    assert_eq!(status, Some(1), "{out}");
    assert!(out.contains("nix-store cannot list the closure"), "{out}");
    assert!(!built.exists(), "built cold under required: {out}");

    let (status, out) = build("on");
    assert_eq!(status, Some(0), "{out}");
    assert!(out.contains("nix-store cannot list the closure"), "{out}");
    let args = sandbox.log("built");
    assert!(!args.contains("sccache-bin="), "{args}");
}

/// `PATH` after `ci.sh`'s `PATH` block, run on `inherited`.
fn ci_sh_path(sandbox: &Sandbox, inherited: &str) -> Vec<PathBuf> {
    let ci = code("deploy/ci.sh");
    let start = ci.find("for _nix_bin in").expect("a PATH block");
    let end = ci.find("\nexport PATH\n").expect("export PATH");
    let out = sandbox
        .command("bash")
        .args(["-c", &format!("{}\nprintf %s \"$PATH\"", &ci[start..end])])
        .env("PATH", inherited)
        .output()
        .expect("bash runs");
    let path = String::from_utf8(out.stdout).unwrap();
    path.split(':').map(PathBuf::from).collect()
}

#[test]
fn ci_sh_finds_the_nix_profile_without_letting_it_shadow_the_toolchain() {
    let sandbox = Sandbox::new();
    let home = sandbox.path("home");
    let cargo = home.join(".cargo/bin");
    // A login shell, or a developer's, already has cargo on PATH.
    let with_cargo = format!("{}:/usr/bin:/bin", cargo.display());
    for inherited in ["/usr/bin:/bin", &with_cargo] {
        let dirs = ci_sh_path(&sandbox, inherited);
        let position = |dir: &Path| {
            dirs.iter()
                .position(|d| d == dir)
                .unwrap_or_else(|| panic!("no {dir:?} on PATH: {dirs:?}"))
        };
        let nix = position(&home.join(".nix-profile/bin"));
        for ahead in [&cargo, &home.join(".local/bin"), Path::new("/usr/bin")] {
            assert!(position(ahead) < nix, "{dirs:?}");
        }
        // A daemon install's nix-store: ci.sh is its own step on the runner
        // and inherits nothing from runner-deps.sh.
        let system = position(&sandbox.path("nix-system/bin"));
        assert!(position(Path::new("/usr/bin")) < system, "{dirs:?}");
    }
}

/// The Containerfile's builder step up to its `cargo build`, as the shell sees
/// it, then the wrapper it chose.
fn containerfile_cache_gate() -> String {
    let joined = code("Containerfile").replace("\\\n", " ");
    let start = joined.find("cache=off; why=;").expect("the cache gate");
    let end = start + joined[start..].find("&& cargo build").expect("cargo build");
    let probe = repo().join("deploy/sccache-probe.sh");
    format!(
        "{} && echo \"wrapper=$RUSTC_WRAPPER\"",
        joined[start..end].replace("/opt/sccache-probe.sh", probe.to_str().unwrap())
    )
}

#[test]
fn containerfile_turns_the_wrapper_on_only_for_a_sccache_that_works() {
    let sandbox = Sandbox::new();
    sandbox.stub("bin/rustc", "#!/bin/sh\n");
    let working = sandbox.stub(
        "working",
        "#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nexec \"$@\"\n",
    );
    let unrunnable = sandbox.stub("unrunnable", "#!/bin/sh\nexit 127\n");
    let dead = sandbox.stub(
        "dead",
        "#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nexit 2\n",
    );
    let gate = containerfile_cache_gate();
    let run = |bin: &Path| {
        let out = sandbox
            .command("sh")
            .args(["-c", &gate])
            .env("PATH", sandbox.stub_path())
            .env("SCCACHE_WEBDAV_ENDPOINT", "http://store.invalid")
            .env("SCCACHE_BIN", bin)
            .output()
            .expect("sh runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let out = run(&working);
    let wrapper = format!("wrapper={}\n", working.display());
    assert!(out.ends_with(&wrapper), "{out}");

    for (bin, why) in [
        (Path::new(""), "SCCACHE_BIN ('') names no executable"),
        (unrunnable.as_path(), "does not execute here"),
        (dead.as_path(), "did not start"),
    ] {
        let out = run(bin);
        assert!(out.contains(why), "{out}");
        assert!(out.ends_with("wrapper=\n"), "{out}");
    }
}

#[test]
fn ci_sh_stops_when_a_required_cache_is_unavailable() {
    let ci = code("deploy/ci.sh");
    for call in [
        "dumpstar_rust_cache_enable || exit 1",
        r#"dumpstar_rust_cache_report "clippy" || exit 1"#,
        r#"dumpstar_rust_cache_report "cargo test" || exit 1"#,
    ] {
        assert!(ci.lines().any(|line| line.trim() == call), "no `{call}`");
    }
}

/// A sccache whose server runs `server` in the foreground, as the real one does
/// under `SCCACHE_NO_DAEMON`, and whose stats show one hit.
fn sccache_with_server(sandbox: &Sandbox, server: &str) {
    sandbox.stub("bin/curl", "#!/bin/sh\n");
    sandbox.stub(
        "sccache-dir/bin/sccache",
        &format!(
            "#!/bin/sh\nif [ \"$SCCACHE_START_SERVER\" = 1 ]; then {server}; fi\n\
             [ \"$1\" = --show-stats ] && printf 'Cache hits 1\\nCache misses 0\\n'\nexit 0\n"
        ),
    );
}

const LIVE_SERVER: &str = "exec sleep 30";
const PORT_TAKEN: &str = "echo 'sccache: error: Address already in use (os error 98)' >&2; exit 2";

/// Run `call` against the stubbed sccache, with `port` as the server's.
fn run_with_server(sandbox: &Sandbox, mode: &str, port: u16, call: &str) -> (Option<i32>, String) {
    sandbox.run_rust_cache(
        call,
        &[
            ("DUMPSTAR_RUST_CACHE", mode),
            ("DUMPSTAR_SCCACHE_SERVER_PORT", &port.to_string()),
            ("TMPDIR", sandbox.dir.path().to_str().unwrap()),
            ("PATH", &sandbox.stub_path()),
        ],
    )
}

/// A port nothing listens on, for a server that never comes up: an answering
/// one could be taken for it before its process had exited.
const SILENT_PORT: u16 = 1;

#[test]
fn required_cache_fails_when_the_server_does_not_start() {
    let sandbox = Sandbox::new();
    sccache_with_server(&sandbox, PORT_TAKEN);
    let (status, out) = run_with_server(
        &sandbox,
        "required",
        SILENT_PORT,
        "dumpstar_rust_cache_enable",
    );
    assert_eq!(status, Some(1), "{out}");
    assert!(out.contains("did not start"), "{out}");
    assert!(out.contains("Address already in use"), "{out}");
}

#[test]
fn a_server_that_does_not_start_compiles_cold_and_says_so() {
    let sandbox = Sandbox::new();
    sccache_with_server(&sandbox, PORT_TAKEN);
    let (status, out) = run_with_server(
        &sandbox,
        "on",
        SILENT_PORT,
        "dumpstar_rust_cache_enable && \
         echo \"STATE=$DUMPSTAR_RUST_CACHE_STATE WRAPPER=${RUSTC_WRAPPER-unset}\"",
    );
    assert_eq!(status, Some(0), "{out}");
    assert!(out.contains("did not start"), "{out}");
    assert!(out.contains("STATE=off WRAPPER=unset"), "{out}");
}

#[test]
fn required_cache_fails_when_the_server_dies_before_the_report() {
    let sandbox = Sandbox::new();
    sccache_with_server(&sandbox, LIVE_SERVER);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (status, out) = run_with_server(
        &sandbox,
        "required",
        listener.local_addr().unwrap().port(),
        "dumpstar_rust_cache_enable && echo \"PID=$DUMPSTAR_SCCACHE_SERVER_PID\" && \
         kill \"$DUMPSTAR_SCCACHE_SERVER_PID\" && wait; dumpstar_rust_cache_report clippy",
    );
    assert_eq!(status, Some(1), "{out}");
    let pid = out
        .lines()
        .find_map(|line| line.strip_prefix("PID="))
        .unwrap_or_default();
    let lost = format!("sccache server (pid {pid}) exited since enable");
    assert!(out.contains(&lost), "{out}");
}

#[test]
fn a_lost_server_under_on_is_said_once_and_the_run_goes_on_cold() {
    let sandbox = Sandbox::new();
    sccache_with_server(
        &sandbox,
        "echo 'sccache: the server said this' >&2; exec sleep 30",
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (status, out) = run_with_server(
        &sandbox,
        "on",
        listener.local_addr().unwrap().port(),
        "dumpstar_rust_cache_enable && kill \"$DUMPSTAR_SCCACHE_SERVER_PID\" && wait; \
         dumpstar_rust_cache_report clippy && echo \"WRAPPER=${RUSTC_WRAPPER-unset}\" && \
         dumpstar_rust_cache_report doctest",
    );
    assert_eq!(status, Some(0), "{out}");
    assert_eq!(out.matches("exited since enable").count(), 1, "{out}");
    assert!(out.contains("the server said this"), "{out}");
    assert!(out.contains("WRAPPER=unset"), "{out}");
    assert!(out.contains("(doctest): not in use"), "{out}");
}

#[test]
fn a_live_server_reports_hits() {
    let sandbox = Sandbox::new();
    sccache_with_server(&sandbox, LIVE_SERVER);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (status, out) = run_with_server(
        &sandbox,
        "required",
        listener.local_addr().unwrap().port(),
        "dumpstar_rust_cache_enable && echo \"PID=$DUMPSTAR_SCCACHE_SERVER_PID\" && \
         dumpstar_rust_cache_report clippy",
    );
    assert_eq!(status, Some(0), "{out}");
    assert!(out.contains("hits=1 misses=0"), "{out}");
    let pid = out
        .lines()
        .find_map(|line| line.strip_prefix("PID="))
        .unwrap_or_else(|| panic!("no PID: {out}"));
    // The server outlived the call, so `output()` did not wait on it: its
    // stdio is not the caller's.
    let killed = Command::new("/bin/kill").arg(pid).status().expect("kill");
    assert!(killed.success(), "server {pid} was not running: {out}");
}

#[test]
fn containerfile_builder_consumes_the_cache() {
    let containerfile = code("Containerfile");
    assert!(containerfile.contains("RUSTC_WRAPPER"));
    assert!(containerfile.contains("from=sccache-bin,source=/,target=/nix,ro"));
    assert!(containerfile.lines().any(|line| line == "ARG SCCACHE_BIN="));
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
fn sccache_flag_a_failed_nix_build_fails_the_install_and_names_sccache() {
    let sandbox = Sandbox::new();
    sandbox.stub(
        "bin/nix",
        "#!/bin/sh\necho 'error: stub build failed' >&2\nexit 1\n",
    );
    sandbox.recording_curl("exit 22");
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("FAILED to install sccache"), "{text}");
    assert!(!text.contains("apt"), "{text}");
}

#[test]
fn sccache_flag_installs_with_nix_build_and_no_curl() {
    let sandbox = Sandbox::new();
    sandbox.stub("bin/nix", WORKING_NIX);
    sandbox.recording_curl("exit 22");
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(out.status.success(), "{text}");
    assert!(
        is_executable(&sandbox.path("sccache-dir/bin/sccache")),
        "{text}"
    );
    assert!(
        sandbox.log("nix.log").contains("build path:.#sccache"),
        "{text}"
    );
    assert_eq!(sandbox.log("curl.log"), "", "{text}");
}

#[test]
fn sccache_flag_uses_a_daemon_installed_nix_that_is_not_on_path() {
    // The CI runner images: /nix is root's, and only a login shell has nix on
    // PATH. Installing a second Nix there fails.
    let sandbox = Sandbox::new();
    sandbox.stub("nix-system/bin/nix", WORKING_NIX);
    sandbox.recording_curl("exit 22");
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(out.status.success(), "{text}");
    assert!(
        is_executable(&sandbox.path("sccache-dir/bin/sccache")),
        "{text}"
    );
    assert_eq!(sandbox.log("curl.log"), "", "{text}");
}

#[test]
fn sccache_builds_without_reading_the_checkouts_git_directory() {
    // testenv mounts a worktree whose .git names a host path the container
    // lacks, so nix must not resolve the flake through git.
    let sandbox = Sandbox::new();
    sandbox.stub("bin/nix", WORKING_NIX);
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(out.status.success(), "{text}");
    let args = sandbox.log("nix.log");
    assert!(args.contains("build path:.#sccache"), "{args}");
    assert!(args.contains("--no-update-lock-file"), "{args}");
    assert_eq!(sandbox.log("nix.cwd"), "flake.lock\nflake.nix\n", "{text}");
}

#[test]
fn sccache_flag_replaces_the_curl_era_install() {
    let sandbox = Sandbox::new();
    sandbox.stub("bin/nix", WORKING_NIX);
    sandbox.stub("sccache-dir/sccache", "#!/bin/sh\n");
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(out.status.success(), "{text}");
    assert!(!sandbox.path("sccache-dir/sccache").exists(), "{text}");
    assert!(
        is_executable(&sandbox.path("sccache-dir/bin/sccache")),
        "{text}"
    );
}

#[test]
fn a_nix_installer_failing_its_checksum_is_not_run() {
    let sandbox = Sandbox::new();
    let ran = sandbox.path("installer-ran");
    let installer = sandbox.stub("installer", &format!("touch '{}'\n", ran.display()));
    let checksum = sandbox.stub("installer.sha256", &"0".repeat(64));
    sandbox.nix_release(&installer, &checksum);
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("checksum"), "{text}");
    assert!(!ran.exists(), "{text}");
    let url = "https://releases.nixos.org/nix/nix-2.35.2/install";
    assert!(sandbox.log("curl.log").contains(url), "{text}");
}

#[test]
fn a_nix_installer_matching_its_checksum_runs_and_its_nix_installs_sccache() {
    let sandbox = Sandbox::new();
    let nix = sandbox.stub("nix.good", WORKING_NIX);
    let installer = sandbox.stub(
        "installer",
        &format!(
            "mkdir -p \"$HOME/.nix-profile/bin\" && cp '{}' \"$HOME/.nix-profile/bin/nix\"\n",
            nix.display()
        ),
    );
    let digest = Command::new("sha256sum")
        .arg(&installer)
        .output()
        .expect("sha256sum runs");
    let digest = String::from_utf8(digest.stdout).unwrap();
    // The published file is the bare digest.
    let checksum = sandbox.stub("installer.sha256", &digest[..64]);
    sandbox.nix_release(&installer, &checksum);
    // As root, the install would create the real /nix.
    sandbox.stub("bin/id", "#!/bin/sh\necho 1000\n");
    fs::create_dir(sandbox.path("home")).unwrap();
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(out.status.success(), "{text}");
    assert!(
        is_executable(&sandbox.path("sccache-dir/bin/sccache")),
        "{text}"
    );
}

#[test]
fn a_root_bootstrap_creates_nix_itself_and_needs_no_build_users_group() {
    // The bare tier container: root, no sudo, no nixbld group.
    let sandbox = Sandbox::new();
    let nix = sandbox.stub(
        "nix.good",
        &WORKING_NIX.replacen(
            '\n',
            "\nprintf %s \"$NIX_CONFIG\" >\"$HOME/nix.config\"\n",
            1,
        ),
    );
    let installer = sandbox.stub(
        "installer",
        &format!(
            "printf %s \"$NIX_CONFIG\" >\"$HOME/installer.config\"\n\
             mkdir -p \"$HOME/.nix-profile/bin\" && cp '{}' \"$HOME/.nix-profile/bin/nix\"\n",
            nix.display()
        ),
    );
    let digest = Command::new("sha256sum")
        .arg(&installer)
        .output()
        .expect("sha256sum runs");
    let digest = String::from_utf8(digest.stdout).unwrap();
    let checksum = sandbox.stub("installer.sha256", &digest[..64]);
    sandbox.nix_release(&installer, &checksum);
    sandbox.stub("bin/id", "#!/bin/sh\necho 0\n");
    // Records a request for /nix instead of touching the real one.
    sandbox.stub(
        "bin/mkdir",
        r#"#!/bin/sh
for a; do
    [ "$a" = /nix ] && echo "$*" >>"$(dirname "$HOME")/mkdir.log" && exit 0
done
exec /bin/mkdir "$@"
"#,
    );
    fs::create_dir(sandbox.path("home")).unwrap();
    let (out, text) = sandbox.runner_deps("--sccache", &[]);
    assert!(out.status.success(), "{text}");
    assert_eq!(sandbox.log("mkdir.log"), "-p -m 0755 /nix\n", "{text}");
    for seen in ["home/installer.config", "home/nix.config"] {
        assert_eq!(sandbox.log(seen), "build-users-group =", "{seen}: {text}");
    }
}

#[test]
fn check_reports_a_missing_sccache() {
    let (out, text) =
        Sandbox::new().runner_deps("--check", &[("DUMPSTAR_RUNNER_DEPS_ONLY", "sccache")]);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("MISSING sccache"), "{text}");
}

#[test]
fn flake_declares_the_sccache_package() {
    assert!(code("flake.nix").contains("packages.${system}.sccache = pkgs.sccache;"));
}

#[test]
fn ci_entry_installs_sccache_before_ci_sh_requires_it() {
    let entry = code(".spira/testenv.toml");
    let entry = entry
        .lines()
        .find_map(|line| line.strip_prefix("ci_entry = "))
        .expect("a ci_entry line");
    let (_, after_install) = entry
        .split_once("runner-deps.sh --sccache && ")
        .unwrap_or_else(|| panic!("ci_entry does not install sccache: {entry}"));
    assert!(after_install.contains("deploy/ci.sh"), "{entry}");
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
fn testenv_rust_tier_installs_sccache_and_requires_the_cache() {
    let argv = testenv_tier_line("rust", "argv");
    let (_, after_fetch) = argv
        .split_once("runner-deps.sh --sccache && ")
        .unwrap_or_else(|| panic!("rust tier does not install sccache: {argv}"));
    assert!(after_fetch.contains("deploy/ci.sh"), "{argv}");
    assert!(argv.contains("xz-utils ca-certificates"), "{argv}");
    let env = testenv_tier_line("rust", "env");
    for setting in [
        r#"DUMPSTAR_RUST_CACHE = "required""#,
        r#"DUMPSTAR_RUNNER_DEPS_ONLY = "base rust nextest sccache""#,
    ] {
        assert!(
            env.contains(setting),
            "rust tier env lacks {setting}: {env}"
        );
    }
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
    // The Nix installer, which `--sccache` runs here too, unpacks a .tar.xz.
    for step in [
        "apt-get install",
        "xz-utils ca-certificates",
        "runner-deps.sh --sccache && ",
    ] {
        let (_, after_step) = argv
            .split_once(step)
            .unwrap_or_else(|| panic!("isolation tier lacks `{step}`: {argv}"));
        assert!(after_step.contains("deploy/ci.sh"), "{argv}");
    }
}
