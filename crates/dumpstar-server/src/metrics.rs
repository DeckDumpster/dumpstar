//! Performance metrics, pushed as OTLP through `dumpster-kit-telemetry` to the product metrics
//! stack (design: sp-typbus). Per-route request count and latency come from the kit's layer;
//! this module adds the search paging meter that decides whether a deep page needs a
//! materialised sort-key table (sp-fk7n2i: build one only if a deep page exceeds ~200 ms).
//!
//! Attributes are bounded enums only — never a tenant id, an email or the query text.

use dumpster_kit_telemetry::{Config, Counter, Histogram, Telemetry};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Milliseconds; includes the 200 ms line the dashboard draws.
const MS_BUCKETS: &[f64] = &[
    5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2500.0, 5000.0,
];
const ROW_BUCKETS: &[f64] = &[0.0, 1.0, 10.0, 25.0, 50.0, 100.0, 250.0, 1000.0, 5000.0];

pub const SEARCH_PAGE_METRIC: &str = "dumpstar_search_page_ms";
pub const SEARCH_TOTALS_METRIC: &str = "dumpstar_search_totals_ms";
pub const SEARCH_ROWS_METRIC: &str = "dumpstar_search_rows";
/// Authenticated requests served, labelled by instance: what the test-environment reaper reads
/// to tell a used environment from an idle one (sp-4wy00r). Never `/health` or an unsigned call.
pub const ACTIVITY_METRIC: &str = "dumpstar_authenticated_requests_total";
/// Requests that entered / left the server, by route. In flight is the difference of the two
/// series (the kit has counters and histograms, no gauge).
pub const STARTED_METRIC: &str = "dumpstar_requests_started_total";
pub const FINISHED_METRIC: &str = "dumpstar_requests_finished_total";
/// The instance name the activity counter is labelled with; `{{INSTANCE}}` in `metrics.env`.
pub const INSTANCE_ENV: &str = "DUMPSTAR_INSTANCE";

/// Paths that answer without a credential or are a machine's probe: never activity.
pub fn is_probe_path(path: &str) -> bool {
    path == "/health" || path == "/api/backup-status" || path.starts_with("/.well-known/")
}

/// The telemetry handle plus the search instruments. `disabled()` records nothing.
#[derive(Clone)]
pub struct Metrics {
    telemetry: Telemetry,
    page_ms: Histogram,
    totals_ms: Histogram,
    rows: Histogram,
    activity: Counter,
    started: Counter,
    finished: Counter,
    instance: Option<String>,
}

/// The page index a request lands on, bucketed: `0`, `1`, `2-4`, `5-19`, `20+`.
pub fn depth_bucket(limit: u32, offset: u32) -> &'static str {
    bucket_of(offset.checked_div(limit).unwrap_or(0))
}

/// How a search was sliced.
#[derive(Clone, Copy)]
pub enum Paging {
    /// `(limit, offset)`.
    Offset(u32, u32),
    /// Keyset cursor; the depth comes from the cursor's page counter.
    Cursor,
    All,
}

fn bucket_of(depth: u32) -> &'static str {
    match depth {
        0 => "0",
        1 => "1",
        2..=4 => "2-4",
        5..=19 => "5-19",
        _ => "20+",
    }
}

impl Metrics {
    pub fn new(telemetry: Telemetry) -> Self {
        Metrics {
            page_ms: telemetry.histogram(
                SEARCH_PAGE_METRIC,
                "Search row-page query time by page depth, sort and paging mode",
                "ms",
                MS_BUCKETS,
            ),
            totals_ms: telemetry.histogram(
                SEARCH_TOTALS_METRIC,
                "Search count-and-value time, first page only",
                "ms",
                MS_BUCKETS,
            ),
            rows: telemetry.histogram(
                SEARCH_ROWS_METRIC,
                "Rows a search request returned",
                "1",
                ROW_BUCKETS,
            ),
            activity: telemetry.counter(
                ACTIVITY_METRIC,
                "Authenticated requests served (MCP, /api, pages); not health checks",
            ),
            started: telemetry
                .counter(STARTED_METRIC, "Requests that entered the server, by route"),
            finished: telemetry.counter(FINISHED_METRIC, "Requests that left the server, by route"),
            instance: std::env::var(INSTANCE_ENV).ok().filter(|v| !v.is_empty()),
            telemetry,
        }
    }

    /// One authenticated request. An unnamed instance records nothing: a series with no
    /// instance label could not be told from another environment's.
    pub fn activity(&self) {
        if let Some(instance) = &self.instance {
            self.activity.add(1, &[("instance", instance.as_str())]);
        }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::new(Telemetry::disabled())
    }

    /// Endpoint from `DUMPSTER_OTLP_METRICS_ENDPOINT`; unset is a no-op. The exporter runs on
    /// its own thread and runtime, so a blocked or saturated request worker never stops a push.
    pub fn from_env() -> Self {
        Self::exporting(Config::from_env(
            "dumpstar",
            "star.dumpster.cards",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    /// `from_env` with an explicit config.
    pub fn exporting(cfg: Config) -> Self {
        if cfg.endpoint.is_none() {
            return Self::new(Telemetry::init(cfg));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("metrics-exporter".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("telemetry: exporter runtime failed: {e}; metrics disabled");
                        let _ = tx.send(Telemetry::disabled());
                        return;
                    }
                };
                rt.block_on(async move {
                    let _ = tx.send(Telemetry::init(cfg.clone()));
                    watch_pushes(cfg, |line| eprintln!("{line}")).await;
                });
            });
        match spawned.ok().and_then(|_| rx.recv().ok()) {
            Some(t) => Self::new(t),
            None => Self::new(Telemetry::disabled()),
        }
    }

    /// A request entered (`true`) or left (`false`) the server.
    pub fn in_flight(&self, route: &str, entered: bool) {
        let c = if entered {
            &self.started
        } else {
            &self.finished
        };
        c.add(1, &[("route", route)]);
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    /// One served search. Totals time is reported only when the totals were computed (the
    /// first page, or the whole result).
    pub fn search(
        &self,
        sort: Option<&str>,
        paging: Paging,
        timings: &dumpstar_db::search::SearchTimings,
        rows: usize,
    ) {
        let (bucket, paged) = match paging {
            Paging::Offset(limit, offset) => (depth_bucket(limit, offset), "offset"),
            Paging::Cursor => (bucket_of(timings.depth), "cursor"),
            Paging::All => ("0", "all"),
        };
        let sort = sort.unwrap_or("default");
        self.page_ms.record(
            timings.rows_ms + timings.copies_ms,
            &[
                ("page_depth_bucket", bucket),
                ("sort", sort),
                ("paged", paged),
            ],
        );
        let first = !matches!(paging, Paging::Offset(_, o) if o > 0);
        if let (true, Some(totals)) = (first, timings.totals_ms) {
            self.totals_ms.record(totals, &[("sort", sort)]);
        }
        self.rows.record(rows as f64, &[("paged", paged)]);
    }
}

/// Minimum gap between two logged push failures.
const FAILURE_LOG_GAP: Duration = Duration::from_secs(300);

/// Rate limiter for the push-failure line: the first failure is logged, then one per gap.
struct FailureLog {
    last: Option<Instant>,
    gap: Duration,
}

impl FailureLog {
    fn new(gap: Duration) -> Self {
        FailureLog { last: None, gap }
    }
    fn due(&mut self, now: Instant) -> bool {
        if self.last.is_none_or(|l| now.duration_since(l) >= self.gap) {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

/// `host:port` of an `http://` OTLP URL, and its path.
fn split_endpoint(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let auth = if auth.contains(':') {
        auth.to_string()
    } else {
        format!("{auth}:80")
    };
    Some((auth, path.to_string()))
}

/// One empty OTLP push (a valid, empty request) so the exporter can say why a push fails: the
/// kit counts a failed push but reports neither the endpoint nor the error.
async fn probe(url: &str, deadline: Duration) -> Result<(), String> {
    let (auth, path) = split_endpoint(url).ok_or("endpoint is not an http:// URL")?;
    let host = auth
        .rsplit_once(':')
        .map_or(auth.as_str(), |(h, _)| h)
        .to_string();
    let attempt = async {
        let mut s = tokio::net::TcpStream::connect(&auth)
            .await
            .map_err(|e| e.to_string())?;
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/x-protobuf\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut head = vec![0u8; 12];
        s.read_exact(&mut head).await.map_err(|e| e.to_string())?;
        let status = String::from_utf8_lossy(&head[9..12]).to_string();
        if status.starts_with('2') {
            Ok(())
        } else {
            Err(format!("HTTP status {status}"))
        }
    };
    tokio::time::timeout(deadline, attempt)
        .await
        .map_err(|_| format!("no answer within {}s", deadline.as_secs()))?
}

/// Probe the endpoint every push interval; log a failure with its endpoint and error,
/// rate-limited, and the recovery once. Runs for the life of the exporter thread.
async fn watch_pushes(cfg: Config, log: impl Fn(&str)) {
    let Some(url) = cfg.endpoint.clone() else {
        return;
    };
    let mut limiter = FailureLog::new(FAILURE_LOG_GAP);
    let mut failing = false;
    let mut tick = tokio::time::interval(cfg.interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        match probe(&url, cfg.deadline).await {
            Ok(()) if failing => {
                failing = false;
                limiter = FailureLog::new(FAILURE_LOG_GAP);
                log(&format!("telemetry: push to {url} recovered"));
            }
            Ok(()) => {}
            Err(e) => {
                failing = true;
                if limiter.due(Instant::now()) {
                    log(&format!("telemetry: push to {url} failed: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_are_not_activity() {
        assert!(is_probe_path("/health"));
        assert!(is_probe_path("/api/backup-status"));
        assert!(is_probe_path("/.well-known/oauth-protected-resource/mcp"));
        assert!(!is_probe_path("/api/collection"));
        assert!(!is_probe_path("/mcp"));
        assert!(!is_probe_path("/"));
    }

    #[test]
    fn depth_buckets() {
        assert_eq!(depth_bucket(60, 0), "0");
        assert_eq!(depth_bucket(60, 60), "1");
        assert_eq!(depth_bucket(60, 120), "2-4");
        assert_eq!(depth_bucket(60, 60 * 4), "2-4");
        assert_eq!(depth_bucket(60, 60 * 5), "5-19");
        assert_eq!(depth_bucket(60, 60 * 19), "5-19");
        assert_eq!(depth_bucket(60, 60 * 20), "20+");
        assert_eq!(depth_bucket(0, 500), "0");
    }

    #[test]
    fn failure_log_is_rate_limited() {
        let mut l = FailureLog::new(Duration::from_secs(300));
        let t = Instant::now();
        assert!(l.due(t));
        assert!(!l.due(t + Duration::from_secs(15)));
        assert!(l.due(t + Duration::from_secs(300)));
    }

    #[test]
    fn endpoint_splits() {
        assert_eq!(
            split_endpoint("http://h:8428/opentelemetry/v1/metrics"),
            Some(("h:8428".into(), "/opentelemetry/v1/metrics".into()))
        );
        assert_eq!(split_endpoint("https://h/x"), None);
    }

    #[tokio::test]
    async fn unreachable_endpoint_logs_endpoint_and_error() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let url = format!("http://127.0.0.1:{port}/opentelemetry/v1/metrics");
        let mut cfg = Config::new("t", "t", "0", Some(url.clone()));
        cfg.interval = Duration::from_millis(20);
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let _ = tokio::time::timeout(
            Duration::from_millis(150),
            watch_pushes(cfg, |l| lines.lock().unwrap().push(l.to_string())),
        )
        .await;
        let lines = lines.into_inner().unwrap();
        assert_eq!(lines.len(), 1, "rate-limited to one line: {lines:?}");
        assert!(
            lines[0].contains(&url) && lines[0].contains("failed"),
            "{lines:?}"
        );
    }

    /// With every request worker blocked, the exporter still pushes.
    #[test]
    fn exporter_pushes_while_the_request_runtime_is_blocked() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(false).unwrap();
        let port = l.local_addr().unwrap().port();
        let mut cfg = Config::new(
            "t",
            "t",
            "0",
            Some(format!("http://127.0.0.1:{port}/opentelemetry/v1/metrics")),
        );
        cfg.interval = Duration::from_millis(50);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for s in l.incoming() {
                let mut s = s.unwrap();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                if buf[..n].windows(4).any(|w| w == b"POST") {
                    let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
                    let _ = tx.send(());
                }
            }
        });
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let m = Metrics::exporting(cfg);
            m.in_flight("/x", true);
            // The only request worker is pegged for the whole wait.
            let got = std::thread::scope(|_| rx.recv_timeout(Duration::from_secs(5)));
            assert!(
                got.is_ok(),
                "no push reached the listener while the runtime was blocked"
            );
        });
    }
}
