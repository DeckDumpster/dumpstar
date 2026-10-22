//! The fetch modes against a fixture site: `cube --group` reads every deck itself and gives the
//! verdict the file path gives, `allocate --group` runs the pack searches, and an expired sign-in
//! is an error that names `login`, never a silent fallback.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::{Arc, Mutex};

use dumpstar_cube::fixture::cube;
use serde_json::{Value, json};

struct Fixture {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
}

/// A site that serves `decks` (group `swu-jumpstart-cube`), their draw cards as search rows,
/// and a token endpoint that grants `refresh_ok`.
fn serve(decks: Vec<Value>, refresh_ok: bool) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (log, b) = (seen.clone(), base.clone());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let (log, b, decks) = (log.clone(), b.clone(), decks.clone());
            std::thread::spawn(move || handle(s, &b, &decks, refresh_ok, &log));
        }
    });
    Fixture { base, seen }
}

fn respond(mut s: TcpStream, code: u16, body: &Value) {
    let body = body.to_string();
    let _ = write!(
        s,
        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn handle(
    mut s: TcpStream,
    base: &str,
    decks: &[Value],
    refresh_ok: bool,
    log: &Mutex<Vec<String>>,
) {
    let mut buf = vec![0u8; 16384];
    let mut n = 0;
    loop {
        let k = s.read(&mut buf[n..]).unwrap_or(0);
        n += k;
        if k == 0 || buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let auth = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("authorization: ")
                .or_else(|| l.strip_prefix("Authorization: "))
        })
        .unwrap_or("")
        .to_string();
    log.lock().unwrap().push(format!("{method} {target}"));
    let url = reqwest::Url::parse(&format!("http://x{target}")).unwrap();
    let q = |k: &str| {
        url.query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.into_owned())
    };
    match (method, url.path()) {
        ("POST", "/token") if refresh_ok => respond(
            s,
            200,
            &json!({"access_token": "tok-fresh", "expires_in": 300, "refresh_token": "refresh-2"}),
        ),
        ("POST", "/token") => respond(s, 400, &json!({"error": "invalid_grant"})),
        ("GET", "/.well-known/oauth-protected-resource") => respond(
            s,
            200,
            &json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]}),
        ),
        (_, p) if p.starts_with("/api/") && auth != "Bearer tok-fresh" => {
            respond(s, 401, &json!({}))
        }
        ("GET", "/api/agent/decks") => {
            let list: Vec<Value> = decks
                .iter()
                .enumerate()
                .filter(|_| q("group").as_deref() == Some("swu-jumpstart-cube"))
                .map(|(i, d)| json!({"id": i + 1, "name": d["deck"]["name"], "group": "swu-jumpstart-cube"}))
                .collect();
            respond(s, 200, &json!({"decks": list}))
        }
        ("GET", p) if p.starts_with("/api/agent/decks/") => {
            let id: usize = p.rsplit('/').next().unwrap().parse().unwrap();
            match decks.get(id - 1) {
                Some(d) => {
                    let mut d = d.clone();
                    for c in d["cards"].as_object_mut().unwrap().values_mut() {
                        c["owned_count"] = json!(2);
                    }
                    d["lines_digest"] =
                        json!(dumpstar_cube::lines_digest(&d["lines"], &d["cards"]));
                    if id == 7 && std::env::var_os("FIXTURE_TAMPER").is_some() {
                        d["cards"]["C7-0"]["cost"] = json!(9);
                    }
                    respond(s, 200, &d)
                }
                None => respond(s, 404, &json!({})),
            }
        }
        ("GET", "/api/agent/search") => {
            let mut rows = Vec::new();
            for d in decks {
                for (id, c) in d["cards"].as_object().unwrap() {
                    if id.starts_with('C') && matches_query(c, &q("q").unwrap_or_default()) {
                        let mut r = c.clone();
                        r["id"] = json!(id);
                        r["owned_count"] = json!(2);
                        r["available_count"] = json!(1);
                        rows.push(r);
                    }
                }
            }
            let from: usize = q("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
            let limit: usize = q("limit").and_then(|c| c.parse().ok()).unwrap_or(25);
            assert!(limit <= 100);
            let end = (from + limit).min(rows.len());
            let next = (end < rows.len()).then(|| end.to_string());
            respond(
                s,
                200,
                &json!({"results": rows[from..end], "total": rows.len(), "next_cursor": next}),
            )
        }
        _ => respond(s, 404, &json!({})),
    }
}

/// The `a:`/`-a:` terms of a query against a card's aspects, the part of the language the pool
/// searches use.
fn matches_query(card: &Value, query: &str) -> bool {
    let aspects: Vec<String> = card["game"]["aspects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_lowercase())
        .collect();
    query
        .split_whitespace()
        .all(|t| match t.strip_prefix("-a:") {
            Some(a) => !aspects.iter().any(|x| x == a),
            None => t
                .strip_prefix("a:")
                .is_none_or(|a| aspects.iter().any(|x| x == a)),
        })
}

fn signed_in(
    dir: &std::path::Path,
    f: &Fixture,
    access: &str,
    expires_at: u64,
) -> std::path::PathBuf {
    let p = dir.join("swu-cube-check.json");
    std::fs::write(
        &p,
        json!({"site": f.base, "client_id": "cid", "token_endpoint": format!("{}/token", f.base),
               "refresh_token": "refresh-1", "access_token": access, "access_expires_at": expires_at,
               "who": "ryan@example.com"})
        .to_string(),
    )
    .unwrap();
    p
}

fn far_future() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

fn run(cfg: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .env("SWU_CUBE_CHECK_CONFIG", cfg)
        .env(
            "SWU_CUBE_CHECK_CLAUDE_CREDENTIALS",
            cfg.with_file_name("no-claude.json"),
        )
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn cube_group_fetches_every_deck_and_gives_the_file_path_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, _) = run(&cfg, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!((code, out.trim()), (0, "ok"));
    let decks = f
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("GET /api/agent/decks/"))
        .count();
    assert_eq!(decks, 20);

    let mut broken = cube();
    broken.pop();
    let f = serve(broken, true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, _) = run(&cfg, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 1);
    assert!(out.contains("FAIL cube-size:"), "{out}");
}

#[test]
fn pack_deck_fetches_one_deck() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, _) = run(&cfg, &["pack", "--deck", "3"]);
    assert_eq!((code, out.trim()), (0, "ok"));
}

#[test]
fn allocate_group_runs_the_pack_searches_itself() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, err) = run(&cfg, &["allocate", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Pack 1: 15 of 15 picks"), "{out}");
    // 20 packs x 4 pool searches, and no file read.
    let searches = f
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("/api/agent/search"))
        .count();
    assert_eq!(searches, 20 * 4);
}

#[test]
fn allocate_plan_runs_its_queries() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let plan = dir.path().join("plan.json");
    std::fs::write(
        &plan,
        json!({"packs": [{"name": "A", "queries": ["t:unit"]}]}).to_string(),
    )
    .unwrap();
    let (code, out, err) = run(&cfg, &["allocate", "--plan", plan.to_str().unwrap()]);
    assert!(code == 0 || code == 1, "{out}{err}");
    assert!(
        f.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("q=t%3Aunit"))
    );
}

#[test]
fn an_expired_sign_in_names_login_and_never_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), false);
    let cfg = signed_in(dir.path(), &f, "old", 1);
    let (code, out, err) = run(&cfg, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 2);
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("swu-cube-check login"), "{err}");
    assert!(!f.seen.lock().unwrap().iter().any(|r| r.contains("/api/")));
}

#[test]
fn a_site_that_rejects_the_token_names_login() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-stale", far_future());
    let (code, _, err) = run(&cfg, &["pack", "--deck", "1"]);
    assert_eq!(code, 2);
    assert!(err.contains("swu-cube-check login"), "{err}");
}

#[test]
fn no_cache_is_not_signed_in() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, err) = run(&dir.path().join("none.json"), &["pack", "--deck", "1"]);
    assert_eq!(code, 2);
    assert!(err.contains("swu-cube-check login"), "{err}");
}

#[test]
fn a_refresh_is_silent_cached_0600_and_prints_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "old", 1);
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, out, err) = run(&cfg, &["pack", "--deck", "1"]);
    assert_eq!(code, 0, "{err}");
    assert!(!format!("{out}{err}").contains("tok-fresh") && !err.contains("refresh-2"));
    assert_eq!(
        std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(
        (
            saved["refresh_token"].as_str(),
            saved["access_token"].as_str()
        ),
        (Some("refresh-2"), Some("tok-fresh"))
    );
    let (code, out, _) = run(&cfg, &["login", "--status"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("ryan@example.com") && !out.contains("tok-"),
        "{out}"
    );
}

fn run_with_claude(
    cfg: &std::path::Path,
    claude: &std::path::Path,
    args: &[&str],
) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .env("SWU_CUBE_CHECK_CONFIG", cfg)
        .env("SWU_CUBE_CHECK_CLAUDE_CREDENTIALS", claude)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn claude_store(dir: &std::path::Path, f: &Fixture, expires_ms: u64) -> std::path::PathBuf {
    let p = dir.join("claude.json");
    std::fs::write(
        &p,
        json!({"mcpOAuth": {"dumpstar|h": {"serverName": "dumpstar", "serverUrl": format!("{}/mcp", f.base),
               "accessToken": "tok-fresh", "refreshToken": "claude-refresh", "expiresAt": expires_ms}}})
        .to_string(),
    )
    .unwrap();
    p
}

#[test]
fn claude_codes_mcp_sign_in_is_used_without_login_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let ms = (far_future() + 3600) * 1000;
    let claude = claude_store(dir.path(), &f, ms);
    let before = std::fs::read_to_string(&claude).unwrap();
    let cfg = dir.path().join("none.json");
    let (code, out, err) =
        run_with_claude(&cfg, &claude, &["login", "--status", "--site", &f.base]);
    assert_eq!(
        (code, out.trim()),
        (0, "Claude Code MCP sign-in (dumpstar)"),
        "{err}"
    );
    let (code, out, _) = run_with_claude(
        &cfg,
        &claude,
        &["cube", "--group", "swu-jumpstart-cube", "--site", &f.base],
    );
    assert_eq!((code, out.trim()), (0, "ok"));
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), before);
    assert!(!cfg.exists());
}

#[test]
fn an_expired_claude_sign_in_says_reconnect_and_falls_through_to_login() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let claude = claude_store(dir.path(), &f, 1);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, err) =
        run_with_claude(&cfg, &claude, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!((code, out.trim()), (0, "ok"), "{err}");
    assert!(err.contains("/mcp") && !err.contains("tok-"), "{err}");
}
