//! One parseable line per finished HTTP request and MCP tool call on stdout (journald via
//! podman), and one line each time a call still running crosses another multiple of the slow
//! threshold, so a stuck call is named while it is stuck.
//!
//! Format: `reqlog phase=<done|inflight> kind=<http|tool> tenant=<id|-> name=<route|tool>
//! args=<comma-joined keys> query="<query string>" status=<code|ok|error> dur_ms=<n>`. Values
//! never hold a secret: arguments are logged as keys, plus the text of a search query.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::response::Response;
use serde_json::Value;

const DEFAULT_SLOW_MS: u64 = 10_000;
static SLOW_MS: AtomicU64 = AtomicU64::new(DEFAULT_SLOW_MS);
const MAX_QUERY: usize = 200;

/// `DUMPSTAR_SLOW_REQUEST_SECS` sets the in-flight threshold (default 10).
pub fn init_from_env() {
    if let Some(s) = std::env::var("DUMPSTAR_SLOW_REQUEST_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
    {
        SLOW_MS.store(s * 1000, Ordering::Relaxed);
    }
}

#[cfg(test)]
pub(crate) fn set_slow_ms(ms: u64) {
    SLOW_MS.store(ms, Ordering::Relaxed);
}

#[cfg(test)]
static CAPTURE: Mutex<Option<Vec<String>>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn capture() -> Vec<String> {
    CAPTURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Vec::new)
        .clone()
}

fn emit(line: String) {
    #[cfg(test)]
    CAPTURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Vec::new)
        .push(line.clone());
    println!("{line}");
}

/// A value safe to put inside double quotes on one line.
fn quote(s: &str) -> String {
    let t: String = s
        .chars()
        .take(MAX_QUERY)
        .map(|c| if c == '"' || c.is_control() { ' ' } else { c })
        .collect();
    format!("\"{t}\"")
}

#[derive(Clone)]
struct Call {
    kind: &'static str,
    tenant: Arc<Mutex<String>>,
    name: String,
    args: String,
    query: String,
}

impl Call {
    fn line(&self, phase: &str, status: &str, dur: Duration) -> String {
        let tenant = self
            .tenant
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        format!(
            "reqlog phase={phase} kind={} tenant={} name={} args={} query={} status={status} dur_ms={}",
            self.kind,
            if tenant.is_empty() { "-" } else { &tenant },
            self.name,
            if self.args.is_empty() {
                "-"
            } else {
                &self.args
            },
            quote(&self.query),
            dur.as_millis()
        )
    }
}

struct Entry {
    call: Call,
    start: Instant,
    crossings: u64,
}

fn registry() -> &'static Mutex<HashMap<u64, Entry>> {
    static R: OnceLock<Mutex<HashMap<u64, Entry>>> = OnceLock::new();
    R.get_or_init(|| {
        std::thread::Builder::new()
            .name("reqlog-watch".into())
            .spawn(watch)
            .expect("spawn reqlog watcher");
        Mutex::new(HashMap::new())
    })
}

fn watch() {
    loop {
        let slow = SLOW_MS.load(Ordering::Relaxed).max(1);
        std::thread::sleep(Duration::from_millis((slow / 4).clamp(5, 1000)));
        let mut due = vec![];
        {
            let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
            for e in map.values_mut() {
                let elapsed = e.start.elapsed();
                let crossed = elapsed.as_millis() as u64 / slow;
                if crossed > e.crossings {
                    e.crossings = crossed;
                    due.push(e.call.line("inflight", "running", elapsed));
                }
            }
        }
        for l in due {
            emit(l);
        }
    }
}

/// Logs the finish line when `finish` is called, and the in-flight lines until then. Dropping it
/// unfinished (a cancelled request) logs `status=dropped`.
pub struct Guard {
    id: u64,
    call: Call,
    start: Instant,
    done: bool,
}

impl Guard {
    fn new(kind: &'static str, tenant: &str, name: &str, args: String, query: String) -> Guard {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let call = Call {
            kind,
            tenant: Arc::new(Mutex::new(tenant.to_string())),
            name: name.to_string(),
            args,
            query,
        };
        let start = Instant::now();
        registry().lock().unwrap_or_else(|e| e.into_inner()).insert(
            id,
            Entry {
                call: call.clone(),
                start,
                crossings: 0,
            },
        );
        Guard {
            id,
            call,
            start,
            done: false,
        }
    }

    fn finish(mut self, status: &str) {
        self.log_done(status);
    }

    fn log_done(&mut self, status: &str) {
        if self.done {
            return;
        }
        self.done = true;
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        emit(self.call.line("done", status, self.start.elapsed()));
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.log_done("dropped");
    }
}

/// The argument keys of a tool call, and the text of its search query (`q` or `query`).
fn shape(args: &Value) -> (String, String) {
    let Some(obj) = args.as_object() else {
        return (String::new(), String::new());
    };
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let query = ["q", "query"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    (keys.join(","), query)
}

/// Run a tool call under the log.
pub fn tool<T, E>(
    tenant: &str,
    name: &str,
    args: &Value,
    run: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let (keys, query) = shape(args);
    let g = Guard::new("tool", tenant, name, keys, query);
    let r = run();
    g.finish(if r.is_ok() { "ok" } else { "error" });
    r
}

/// Filled by the layer that resolves the tenant, so the outer request log can name it.
#[derive(Clone, Default)]
pub struct TenantSlot(Arc<Mutex<String>>);

impl TenantSlot {
    pub fn set(&self, tenant: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = tenant.to_string();
    }
}

/// Axum middleware: log every HTTP request. The path is logged without its query; the query string is logged with its values for `q` only.
pub async fn http(mut req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let name = format!("{} {}", req.method(), req.uri().path());
    let (keys, query) = query_shape(req.uri().query().unwrap_or(""));
    let slot = TenantSlot::default();
    req.extensions_mut().insert(slot.clone());
    let mut g = Guard::new("http", "", &name, keys, query);
    g.call.tenant = slot.0.clone();
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(g.id)
        .and_modify(|e| e.call.tenant = slot.0.clone());
    let resp = next.run(req).await;
    let status = resp.status().as_u16().to_string();
    g.finish(&status);
    resp
}

/// Keys of a URL query string, and the text of `q`, percent-decoding left alone.
fn query_shape(qs: &str) -> (String, String) {
    let mut keys = vec![];
    let mut q = String::new();
    for pair in qs.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == "q" {
            q = v.to_string();
        }
        keys.push(k);
    }
    keys.sort_unstable();
    (keys.join(","), q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_logs_done_line_and_slow_call_logs_inflight_first() {
        set_slow_ms(50);
        let r: Result<(), ()> = tool(
            "tenant-t1",
            "slow_tool_t1",
            &serde_json::json!({"q": "aspect:vigilance", "token": "SECRET"}),
            || {
                std::thread::sleep(Duration::from_millis(220));
                Ok(())
            },
        );
        assert!(r.is_ok());
        let lines: Vec<String> = capture()
            .into_iter()
            .filter(|l| l.contains("name=slow_tool_t1"))
            .collect();
        let inflight = lines
            .iter()
            .position(|l| l.contains("phase=inflight"))
            .unwrap();
        let done = lines.iter().position(|l| l.contains("phase=done")).unwrap();
        assert!(inflight < done, "{lines:?}");
        let d = &lines[done];
        assert!(d.contains("kind=tool tenant=tenant-t1 name=slow_tool_t1 args=q,token query=\"aspect:vigilance\" status=ok dur_ms="), "{d}");
        assert!(!lines.iter().any(|l| l.contains("SECRET")));
    }
}
