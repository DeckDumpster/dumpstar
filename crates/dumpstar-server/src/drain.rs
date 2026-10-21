//! The draining state: on the drain signal new requests are refused with a 503 and `Retry-After`,
//! requests already in flight run to completion, and the server stops when the last one ends or
//! the drain timeout passes, whichever comes first. A deploy then costs a brief refusal window
//! rather than a dropped request (sp-4kt4e8; the 502s of sp-slm2ib).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use dumpster_kit_telemetry::Telemetry;

/// Seconds a refused client is told to wait. A restart is a few seconds; this is the floor.
const RETRY_AFTER_SECS: &str = "5";

const DRAIN_SECS_BUCKETS: &[f64] = &[0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0];
const INFLIGHT_BUCKETS: &[f64] = &[0.0, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0];

pub const DRAIN_SECONDS_METRIC: &str = "dumpstar_drain_seconds";
pub const DRAIN_COMPLETED_METRIC: &str = "dumpstar_drain_completed_total";
pub const DRAIN_REFUSED_METRIC: &str = "dumpstar_drain_refused_total";
pub const DRAIN_INFLIGHT_METRIC: &str = "dumpstar_drain_inflight_at_start";

/// Shared between the gate middleware and the drain task.
#[derive(Default)]
pub struct Drain {
    draining: AtomicBool,
    inflight: AtomicUsize,
    completed: AtomicU64,
    refused: AtomicU64,
}

struct InFlight(Arc<Drain>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
        if self.0.draining.load(Ordering::SeqCst) {
            self.0.completed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Drain {
    pub fn new() -> Arc<Self> {
        Arc::new(Drain::default())
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }
}

/// Outermost layer: counts requests in flight, and once draining answers every new one — `/health`
/// and `/mcp` included — with 503 and `Retry-After`.
pub fn layer(app: Router, drain: Arc<Drain>) -> Router {
    app.layer(middleware::from_fn_with_state(drain, gate))
}

async fn gate(State(drain): State<Arc<Drain>>, req: Request, next: Next) -> Response {
    // Count first, then look at the flag: the drain task sets the flag and only then reads the
    // count, so a request that slips in either shows up in the count or sees the flag.
    drain.inflight.fetch_add(1, Ordering::SeqCst);
    if drain.draining.load(Ordering::SeqCst) {
        drain.inflight.fetch_sub(1, Ordering::SeqCst);
        drain.refused.fetch_add(1, Ordering::SeqCst);
        let mut res = (StatusCode::SERVICE_UNAVAILABLE, "draining\n").into_response();
        res.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_static(RETRY_AFTER_SECS),
        );
        return res;
    }
    let _guard = InFlight(drain);
    next.run(req).await
}

struct DrainMetrics {
    seconds: dumpster_kit_telemetry::Histogram,
    inflight: dumpster_kit_telemetry::Histogram,
    completed: dumpster_kit_telemetry::Counter,
    refused: dumpster_kit_telemetry::Counter,
}

impl DrainMetrics {
    fn new(t: &Telemetry) -> Self {
        DrainMetrics {
            seconds: t.histogram(
                DRAIN_SECONDS_METRIC,
                "Seconds from the drain signal to exit",
                "s",
                DRAIN_SECS_BUCKETS,
            ),
            inflight: t.histogram(
                DRAIN_INFLIGHT_METRIC,
                "Requests in flight when draining started",
                "1",
                INFLIGHT_BUCKETS,
            ),
            completed: t.counter(
                DRAIN_COMPLETED_METRIC,
                "Requests that finished during a drain",
            ),
            refused: t.counter(
                DRAIN_REFUSED_METRIC,
                "Requests refused with 503 during a drain",
            ),
        }
    }
}

/// Serve `app` on `listener` until `signal` resolves, then drain for at most `timeout`.
pub async fn serve_draining(
    listener: tokio::net::TcpListener,
    app: Router,
    drain: Arc<Drain>,
    telemetry: &Telemetry,
    timeout: Duration,
    signal: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let metrics = DrainMetrics::new(telemetry);
    let server = axum::serve(listener, layer(app, drain.clone()));
    let drained = async {
        signal.await;
        let started = Instant::now();
        drain.draining.store(true, Ordering::SeqCst);
        let at_start = drain.in_flight();
        eprintln!("draining: {at_start} in flight");
        while drain.in_flight() > 0 && started.elapsed() < timeout {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let secs = started.elapsed().as_secs_f64();
        let completed = drain.completed.load(Ordering::SeqCst);
        let refused = drain.refused.load(Ordering::SeqCst);
        let cut = drain.in_flight();
        eprintln!(
            "drained in {secs:.1}s ({completed} completed, {refused} refused{})",
            if cut > 0 {
                format!(", {cut} cut at the timeout")
            } else {
                String::new()
            }
        );
        metrics.seconds.record(secs, &[]);
        metrics.inflight.record(at_start as f64, &[]);
        metrics.completed.add(completed, &[]);
        metrics.refused.add(refused, &[]);
        telemetry.shutdown().await;
    };
    tokio::select! {
        r = server.into_future() => r.map_err(Into::into),
        _ = drained => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn get_raw(addr: std::net::SocketAddr, path: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out).await;
        out
    }

    fn router(slow: Duration) -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route(
                "/slow",
                get(move || async move {
                    tokio::time::sleep(slow).await;
                    "done"
                }),
            )
    }

    struct Harness {
        addr: std::net::SocketAddr,
        stop: tokio::sync::oneshot::Sender<()>,
        server: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    async fn start(slow: Duration, timeout: Duration) -> Harness {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            serve_draining(
                listener,
                router(slow),
                Drain::new(),
                &Telemetry::disabled(),
                timeout,
                async {
                    let _ = rx.await;
                },
            )
            .await
        });
        Harness { addr, stop, server }
    }

    #[tokio::test]
    async fn in_flight_finishes_new_requests_get_503_then_exit() {
        let h = start(Duration::from_millis(800), Duration::from_secs(10)).await;
        assert!(get_raw(h.addr, "/health").await.starts_with("HTTP/1.1 200"));
        let slow = tokio::spawn(get_raw(h.addr, "/slow"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        h.stop.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let refused = get_raw(h.addr, "/slow").await;
        assert!(refused.starts_with("HTTP/1.1 503"), "{refused}");
        assert!(
            refused.to_lowercase().contains("retry-after: 5"),
            "{refused}"
        );
        let health = get_raw(h.addr, "/health").await;
        assert!(health.starts_with("HTTP/1.1 503"), "{health}");

        let done = slow.await.unwrap();
        assert!(
            done.starts_with("HTTP/1.1 200") && done.ends_with("done"),
            "{done}"
        );
        tokio::time::timeout(Duration::from_secs(5), h.server)
            .await
            .expect("the server exits once the last request ends")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_request_longer_than_the_timeout_is_cut() {
        let h = start(Duration::from_secs(30), Duration::from_millis(500)).await;
        let slow = tokio::spawn(get_raw(h.addr, "/slow"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let began = Instant::now();
        h.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), h.server)
            .await
            .expect("the server exits at the timeout")
            .unwrap()
            .unwrap();
        assert!(began.elapsed() < Duration::from_secs(3));
        slow.abort();
    }
}
