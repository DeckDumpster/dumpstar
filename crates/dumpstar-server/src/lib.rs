//! `dumpstar-server` — the Axum HTTP application for Dumpstar.
//!
//! Holds one collection-database connection per tenant (with the shared
//! catalog attached) behind a mutex; a tenant's requests are serialised,
//! which is fine for a personal collection tracker. The JSON API lives under
//! `/api`; every other path is served from the SvelteKit static build,
//! falling back to `index.html` so the SPA handles client-side routing.
//!
//! Which tenant a request is served as is decided by [`tenant`], once, in a
//! middleware — and by default there is only one, exactly as before.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Router, middleware, routing::get};
use rusqlite::Connection;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{NotForContentType, Predicate, SizeAbove};
use tower_http::services::{ServeDir, ServeFile};

use dumpstar_core::query::KeywordRegistry;
use dumpstar_db::DbError;
use dumpstar_db::search_meta::SearchFlag;

pub mod drain;
mod mcp;
mod metrics;
mod routes;

use dumpster_kit_mcp_auth::{McpAuth, McpAuthConfig};
pub use dumpster_kit_oidc::OidcConfig;
use dumpster_kit_oidc::{Oidc, SessionUser};
use dumpster_kit_telemetry::Telemetry;
use dumpster_kit_tenancy::tenant::TenantId;
use dumpster_kit_tenancy::{AccessState, HttpError, Tenants};
pub use dumpster_kit_tenancy::{access, tenant};

tokio::task_local! {
    /// The tenant an OIDC-signed-in request resolved to. Only [`session_tenant`] scopes it, and
    /// the only source of a [`TenantId`] is [`Tenants::resolve_session`].
    static SESSION_TENANT: TenantId;
}

/// The tenant this request is served as: the OIDC session's, else the Access identity's.
fn current_tenant() -> Result<TenantId, HttpError> {
    SESSION_TENANT
        .try_with(TenantId::clone)
        .or_else(|_| tenant::current())
}

/// Paths served without a session: the liveness probe and the backup-freshness marker that
/// `alarm-status.sh` reads from localhost. Neither carries tenant data.
const OIDC_BYPASS_PATHS: [&str; 2] = ["/health", "/api/backup-status"];

/// Shared application state: the collection databases this process may serve
/// plus the immutable search registry/flags loaded once at startup.
///
/// Note what is *not* here: a connection. Reaching a database goes through
/// [`Tenants::connection`] with a [`tenant::TenantId`] that only the
/// resolution middleware can mint, so there is no ambient default collection
/// for a handler to pick up by accident.
#[derive(Clone)]
pub struct AppState {
    tenants: Arc<Tenants>,
    registry: Arc<KeywordRegistry>,
    flags: Arc<Vec<SearchFlag>>,
    /// The data dir — read by `/api/backup-status` for the `.backup-last-ok`
    /// freshness marker the host-side Layer 1 checker writes (ivq.5).
    data_dir: Arc<PathBuf>,
    /// Pocket ID sign-in. When present it is the only way in; Access is not consulted.
    pub(crate) oidc: Option<Arc<Oidc>>,
    /// Cloudflare Access JWT validation state (JWKS cache + config), used only when `oidc` is
    /// absent — the transitional mode, and what the container harnesses run.
    pub(crate) access: Option<Arc<AccessState>>,
    /// The agent-tool endpoint (`/mcp`), when configured. Needs `oidc`: Pocket ID is its
    /// authorization server.
    pub(crate) mcp: Option<McpMount>,
    /// OTLP metrics; a no-op unless `DUMPSTER_OTLP_METRICS_ENDPOINT` is set.
    pub(crate) metrics: metrics::Metrics,
}

/// What `/mcp` is served from: the token validator and the tool set.
#[derive(Clone)]
pub(crate) struct McpMount {
    auth: Arc<McpAuth>,
    tools: Arc<mcp::DumpstarTools>,
    resolver: Arc<mcp::Resolver>,
    telemetry: Telemetry,
}

impl AppState {
    /// Where the catalog job keeps the retrieved set logos and art crops.
    fn art_dir(&self) -> PathBuf {
        self.data_dir.join("art")
    }
}

/// An error rendered as an HTTP response. `DbError::NotFound` → 404,
/// `DbError::Conflict` → 409, `DbError::Import` → 400, everything else → 500.
#[derive(Debug)]
pub struct AppError(StatusCode, String);

impl AppError {
    fn internal(msg: impl Into<String>) -> Self {
        AppError(StatusCode::INTERNAL_SERVER_ERROR, msg.into())
    }

    /// A 400 whose body is JSON the frontend reads: `{error, position}` for a
    /// query-language parse error, `{error}` alone for everything else.
    fn bad_request(body: impl Into<String>) -> Self {
        AppError(StatusCode::BAD_REQUEST, body.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        let code = match e {
            DbError::NotFound(_) => StatusCode::NOT_FOUND,
            DbError::Conflict(_) => StatusCode::CONFLICT,
            DbError::Import(_) => StatusCode::BAD_REQUEST,
            DbError::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        AppError(code, e.to_string())
    }
}

impl From<HttpError> for AppError {
    fn from(HttpError(code, body): HttpError) -> Self {
        AppError(code, body)
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        AppError::internal(format!("background task failed: {e}"))
    }
}

/// Run a blocking database closure on the connection without holding the
/// async executor. The closure receives `&mut Connection` so it works with
/// both read and write repository functions.
///
/// This is the only route from a handler to a database, and it takes the
/// tenant from the request scope rather than from anything the handler
/// passes in — that is what makes cross-tenant access unreachable from
/// route code rather than merely unusual. A request that never went through
/// the resolution middleware errors here; it does not get a default.
async fn blocking<T, F>(state: &AppState, f: F) -> Result<T, AppError>
where
    F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    T: Send + 'static,
{
    let id = current_tenant()?;
    let tenants = state.tenants.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = tenants.connection(&id)?;
        let mut guard = conn.lock().expect("connection mutex poisoned");
        f(&mut guard)
    })
    .await?;
    Ok(result?)
}

/// Responses smaller than this are sent uncompressed. (A response of exactly
/// this size is compressed — `SizeAbove` is inclusive — and a response whose
/// size is not known up front is compressed too, since there is nothing to
/// compare.)
///
/// Compression is not free — it costs CPU on this box and a decompress on the
/// client, and it replaces a known `content-length` with a chunked body. Under
/// ~1 KB there is nothing to buy with that: the response already fits inside a
/// single ~1460-byte TCP segment, so squeezing it does not remove a round trip,
/// and gzip's own header/trailer can make a very short body *larger*. 1 KB is
/// the first size where the saving is real, and it leaves every small JSON
/// answer in the API (`/api/backup-status`, a 204, an error envelope)
/// untouched.
///
/// The big payloads this exists for are three to four orders of magnitude
/// above it — the catalog-wide `/api/collection/search` is ~44 MB raw — so the
/// exact threshold only decides the fate of responses where it does not matter.
const COMPRESS_MIN_BYTES: u16 = 1024;

/// Response compression for the whole app.
///
/// Every response left this process uncompressed until now, including the
/// catalog-wide search body. JSON of that shape compresses about 9x, which is
/// the difference between a result set the browser can hold and one it cannot.
///
/// Three decisions are baked in here:
///
/// * **Algorithms: gzip and br, chosen by the client.** The layer reads
///   `Accept-Encoding` and picks; it never forces one. Brotli usually beats
///   gzip on JSON and every current browser offers it, while gzip is the
///   universal floor for anything else (`curl`, a script, an old client). A
///   request that offers neither — or no `Accept-Encoding` at all — gets the
///   same valid uncompressed bytes it got before. zstd and deflate are
///   deliberately absent: deflate is redundant with gzip and nothing prefers
///   it, and zstd's marginal win over br does not pay for a second native
///   dependency.
/// * **Never images.** `/sym`, `/rarity` and the rest serve PNG and JPEG,
///   which are already compressed; running them through gzip spends CPU to
///   make the payload *bigger*. `NotForContentType::IMAGES` excludes anything
///   `image/*`, which sweeps up `image/svg+xml` too — those are text and would
///   compress well, but every SVG shipped here is under a kilobyte and would
///   fall below the size floor anyway.
/// * **A size floor.** See [`COMPRESS_MIN_BYTES`].
///
/// It sits on the outermost router, so it covers the SPA shell and the
/// SvelteKit bundle under `/_app` as well as `/api`. Nothing here buffers a
/// whole body to compress it — the layer wraps the body and compresses as it
/// streams — so `/api/export/*`, which hands back a full collection dump,
/// costs no more memory than it did before.
// The predicate type is an opaque `And` tower of the four rules below; naming
// it buys nothing. `Clone`/`Send`/`Sync` come from `Predicate` itself.
fn compression() -> CompressionLayer<impl Predicate + 'static> {
    CompressionLayer::new().gzip(true).br(true).compress_when(
        SizeAbove::new(COMPRESS_MIN_BYTES)
            .and(NotForContentType::IMAGES)
            .and(NotForContentType::SSE)
            .and(NotForContentType::GRPC),
    )
}

/// Mark every `/api` response `private, no-store`.
///
/// The bodies under `/api` are one tenant's data, chosen by a credential the
/// URL does not carry: the same `GET /api/collection` is a different answer for
/// each caller. A shared cache between the origin and the browser (Cloudflare,
/// a corporate proxy) keys on the URL, so any response it was allowed to keep
/// could be served to the next tenant. This is set on the way out for every
/// response under the prefix, errors and the public routes included, and
/// overwrites whatever a handler set: a handler cannot opt out.
async fn api_responses_are_private(
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let is_api = req.uri().path() == "/api" || req.uri().path().starts_with("/api/");
    let mut resp = next.run(req).await;
    if is_api {
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    resp
}

/// Require an OIDC session on every path but the bypass list and `/auth/*`. A browser
/// navigating is sent to Pocket ID; a signed-in user outside the required group gets a 403.
/// An `/api` call without a session is a 401, never a redirect: the SPA's `fetch` cannot follow
/// a cross-origin sign-in page.
async fn sign_in_gate(
    State(oidc): State<Option<Arc<Oidc>>>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let Some(oidc) = oidc else {
        return next.run(req).await;
    };
    let is_api = req.uri().path() == "/api" || req.uri().path().starts_with("/api/");
    let resp = Oidc::layer(State(oidc), req, next).await;
    if is_api && resp.status().is_redirection() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    resp
}

/// Resolve the signed-in user's tenant from their session's email, then serve the request as it.
/// Runs inside [`sign_in_gate`], which put the [`SessionUser`] in the request.
async fn session_tenant(
    State(tenants): State<Arc<Tenants>>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Result<Response, HttpError> {
    let user = req
        .extensions()
        .get::<SessionUser>()
        .cloned()
        .ok_or_else(|| HttpError(StatusCode::UNAUTHORIZED, "not signed in".into()))?;
    let id = tenants.resolve_session(&user)?;
    Ok(SESSION_TENANT.scope(id, next.run(req)).await)
}

/// Build the Axum router. `/health` and `/api/*` are handled by Rust; the
/// SvelteKit bundle under `/_app` and `/robots.txt` are served as files;
/// every other path returns the SPA's `index.html` so SvelteKit handles
/// client-side routing.
fn app(state: AppState, static_dir: PathBuf, data_dir: PathBuf) -> Router {
    let index_html = std::fs::read_to_string(static_dir.join("index.html")).unwrap_or_else(|_| {
        "<!doctype html><title>Dumpstar</title><body>\
             Frontend not built — run <code>npm run build</code> in frontend/.\
             </body>"
            .to_string()
    });
    let spa = move || {
        let html = index_html.clone();
        async move { axum::response::Html(html) }
    };
    // `route_layer`, not `layer`: resolution runs for API routes that match,
    // and an unmatched `/api/...` path still falls through to the SPA
    // fallback exactly as it did before.
    //
    // Layer ordering: the outermost `route_layer` runs first. The access
    // layer (outermost) verifies the JWT before the tenant layer (inner)
    // reads the tenant header.
    //
    // Public `/api` routes (see `routes::public_api_router`) are mounted
    // separately, without the auth layers, so they remain reachable from
    // plain localhost — e.g. `alarm-status.sh` reading `/api/backup-status`.
    let authenticated_api = match (&state.oidc, &state.access) {
        (Some(_), _) => routes::authenticated_api_router().route_layer(
            middleware::from_fn_with_state(state.tenants.clone(), session_tenant),
        ),
        (None, Some(access)) => routes::authenticated_api_router()
            .route_layer(middleware::from_fn_with_state(
                state.tenants.clone(),
                tenant::layer,
            ))
            .route_layer(middleware::from_fn_with_state(
                access.clone(),
                access::layer,
            )),
        (None, None) => unreachable!("serve() refuses to start with neither sign-in configured"),
    };
    let public_api = routes::public_api_router();
    let state_for_gate = state.oidc.clone();
    let mcp_routes = state.mcp.as_ref().map(|m| {
        let endpoint = dumpster_kit_mcp::router_with_telemetry(
            "dumpstar",
            m.tools.clone(),
            m.resolver.clone(),
            dumpster_kit_mcp::Limits::default(),
            &m.telemetry,
        )
        .layer(middleware::from_fn_with_state(
            m.auth.clone(),
            McpAuth::layer,
        ));
        Router::new()
            .nest("/mcp", endpoint)
            .merge(m.auth.metadata_router())
    });
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api", authenticated_api)
        .nest("/api", public_api)
        .nest_service("/_app", ServeDir::new(static_dir.join("_app")))
        // Trimmed-and-resized set symbol glyphs written by the ingest
        // pipeline's symbols phase. Lives on the data volume so it
        // rebuilds from upstream alongside shared.sqlite, not in the
        // baked-in image static dir.
        // The official aspect icons, copied from data/remedial/aspects/ into
        // the static build.
        .nest_service("/aspects", ServeDir::new(static_dir.join("aspects")))
        // Official set logos and art crops, retrieved once by the catalog job
        // into the data volume (dumpstar_catalog::art) — never the publisher's
        // CDN at page load.
        .nest_service("/art", ServeDir::new(data_dir.join("art")))
        // The official inline game icons, copied from data/remedial/
        // inline-icons/ into the static build.
        .nest_service("/icons", ServeDir::new(static_dir.join("icons")))
        .nest_service("/sym", ServeDir::new(data_dir.join("symbols")))
        // Sealed-product pictures fetched at catalog refresh. Pages read these;
        // nothing at runtime reaches the source CDN.
        .nest_service("/sealed-img", ServeDir::new(data_dir.join("sealed-images")))
        .route_service("/robots.txt", ServeFile::new(static_dir.join("robots.txt")))
        // The tab and home-screen icons. Without a route each fell through to
        // the SPA fallback and answered 200 text/html, so the browser fetched
        // the app shell as an image.
        .route_service(
            "/favicon.svg",
            ServeFile::new(static_dir.join("favicon.svg")),
        )
        .route_service(
            "/favicon-32.png",
            ServeFile::new(static_dir.join("favicon-32.png")),
        )
        .route_service(
            "/apple-touch-icon.png",
            ServeFile::new(static_dir.join("apple-touch-icon.png")),
        )
        .with_state(state.clone())
        .merge(state.oidc.as_ref().map(|o| o.router()).unwrap_or_default())
        .merge(mcp_routes.unwrap_or_default())
        .fallback(get(spa))
        .layer(middleware::from_fn_with_state(state_for_gate, sign_in_gate))
        .layer(middleware::from_fn(api_responses_are_private))
        .layer(dumpster_kit_telemetry::RouteMetricsLayer::new(
            state.metrics.telemetry(),
        ))
        // Outermost, so it sees the final response of every route above.
        .layer(compression())
}

/// Everything `dumpstar serve` needs to stand the HTTP app up.
pub struct ServeConfig {
    /// The directory holding one database per tenant, and the registry that
    /// says which of them an identity is served from.
    pub tenants_dir: PathBuf,
    pub registry_db: PathBuf,
    pub shared_db: PathBuf,
    pub static_dir: PathBuf,
    pub data_dir: PathBuf,
    pub host: IpAddr,
    pub port: u16,
    /// Pocket ID sign-in (`DUMPSTAR_OIDC_*`). Takes precedence over `access`; a discovery or
    /// JWKS failure is a startup failure. See [`OidcConfig::from_env`].
    pub oidc: Option<OidcConfig>,
    /// Cloudflare Access configuration, consulted only when `oidc` is absent. One of the two is
    /// mandatory: there is no mode with neither.
    pub access: Option<access::AccessConfig>,
    /// The agent-tool endpoint's own URL (`DUMPSTAR_MCP_RESOURCE`, e.g. `https://star.dumpster.cards/mcp`),
    /// the `aud` its access tokens must carry. `None` serves no `/mcp`. Requires `oidc`.
    pub mcp_resource: Option<String>,
    /// How long a drain waits for requests in flight (`DUMPSTAR_DRAIN_TIMEOUT_SECS`).
    pub drain_timeout: std::time::Duration,
}

/// Paths the browser sign-in gate lets through when `/mcp` is served: the endpoint itself
/// (which validates its own bearer token) and the protected-resource metadata clients read first.
const MCP_BYPASS_PATHS: [&str; 2] = ["/mcp", "/.well-known/oauth-protected-resource"];

/// Start the HTTP server. The shared catalog is opened up front — so a missing
/// catalog (`dumpstar setup` not run) fails at startup rather than on the
/// first request.
pub async fn serve(cfg: ServeConfig) -> anyhow::Result<()> {
    // Idempotent shared-catalog convergence on startup. `dumpstar setup` and
    // the nightly `dumpstar-lake-derive shared` normally own shared schema —
    // the refresh used to be on that list and is not since pd-lunn, because
    // it opens the catalog read-only now — but a binary upgrade can ship a
    // data-only migration (e.g. seeding a new variant) that must be applied
    // before the server starts serving requests.
    //
    // `open_shared_for_serving`, not `open_shared`: it asks READ-ONLY whether
    // this catalog already carries this build's convergence fingerprint and,
    // on a match, never takes the write lock at all (pd-dzu5). It used to,
    // unconditionally and on every start — the search-metadata reconcile it
    // used to make here was a DELETE and a few hundred INSERTs each time —
    // which put a restart in a race with the nightly derive it can only lose.
    // That reconcile is now part of the convergence itself, so it happens on
    // exactly the starts that have something to converge.
    //
    // The search registry is read off the catalog here too: it is catalog
    // data, the same for every tenant, so it is loaded once from the one
    // shared database rather than through whichever collection happens to
    // be open.
    let (registry, flags) = {
        let shared = dumpstar_db::open_shared_for_serving(&cfg.shared_db)?;
        (
            Arc::new(dumpstar_db::search_meta::load_registry(&shared)?),
            Arc::new(dumpstar_db::search_meta::load_flags(&shared)?),
        )
    };
    // Primes the provider's discovery and JWKS; a failed fetch is a startup failure.
    if cfg.mcp_resource.is_some() && cfg.oidc.is_none() {
        anyhow::bail!(
            "refusing to start: DUMPSTAR_MCP_RESOURCE needs Pocket ID sign-in (DUMPSTAR_OIDC_*), which is the /mcp authorization server"
        );
    }
    let mcp_cfg = match (&cfg.mcp_resource, &cfg.oidc) {
        (Some(resource), Some(o)) => Some(McpAuthConfig {
            issuer: o.issuer.clone(),
            resource: resource.clone(),
            required_group: o.required_group.clone(),
            clock_skew: o.clock_skew,
            jwks_refetch_min: o.jwks_refetch_min,
        }),
        _ => None,
    };
    let oidc = match cfg.oidc {
        Some(mut c) => {
            c.bypass_paths
                .extend(OIDC_BYPASS_PATHS.iter().map(|p| p.to_string()));
            if mcp_cfg.is_some() {
                c.bypass_paths
                    .extend(MCP_BYPASS_PATHS.iter().map(|p| p.to_string()));
            }
            Some(Oidc::discover(c).await?)
        }
        None => None,
    };
    let access = match (&oidc, cfg.access) {
        (None, Some(a)) => Some(AccessState::new(a).await?),
        (None, None) => anyhow::bail!(
            "refusing to start: no sign-in is configured (set DUMPSTAR_OIDC_* — see deploy/TENANTS.md)"
        ),
        (Some(_), _) => None,
    };
    let tenants = Tenants::new(
        dumpstar_db::app::app().clone(),
        cfg.tenants_dir,
        cfg.shared_db,
        &cfg.registry_db,
        dumpstar_db::user_spec(),
    )?;
    let tenants = Arc::new(tenants);
    let mcp = match mcp_cfg {
        Some(c) => {
            let telemetry = Telemetry::init(dumpster_kit_telemetry::Config::from_env(
                "dumpstar",
                "dumpstar",
                env!("CARGO_PKG_VERSION"),
            ));
            let auth = McpAuth::discover(c, tenants.clone(), &telemetry).await?;
            let (tools, resolver) = mcp::tools(tenants.clone(), registry.clone(), flags.clone())
                .map_err(anyhow::Error::msg)?;
            Some(McpMount {
                auth,
                tools,
                resolver,
                telemetry,
            })
        }
        None => None,
    };
    let state = AppState {
        tenants,
        registry,
        flags,
        data_dir: Arc::new(cfg.data_dir.clone()),
        oidc,
        access,
        mcp,
        metrics: metrics::Metrics::from_env(),
    };
    let addr = SocketAddr::new(cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("dumpstar serving on http://{addr}");
    // SIGTERM (what `systemctl stop` and `podman stop` send) starts a drain: new requests get 503,
    // requests in flight finish, and the server exits when they have or the timeout passes.
    let telemetry = state.metrics.telemetry().clone();
    drain::serve_draining(
        listener,
        app(state, cfg.static_dir, cfg.data_dir),
        drain::Drain::new(),
        &telemetry,
        cfg.drain_timeout,
        shutdown_signal(),
    )
    .await
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Err(_) => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};
    use dumpster_kit_tenancy::testing::TestAccessFixture;
    use tower::ServiceExt;

    /// A catalog holding one printing (`SOR-1-normal`), plus the static
    /// dir the router serves the SPA shell from.
    fn seed(dir: &std::path::Path) -> PathBuf {
        let shared = dir.join("shared.sqlite");
        let mut c = dumpstar_db::open_shared(&shared).unwrap();
        dumpstar_db::search_meta::reconcile(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO sets (set_code, name, release_date, sort_order) \
               VALUES ('SOR', 'Spark of Rebellion', '2024-03-08', 1); \
             INSERT INTO cards (card_id, name, card_type, cost, aspects) \
               VALUES ('SOR-1', 'Luke Skywalker', 'Unit', 3, '[\"Vigilance\"]'); \
             INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
               VALUES ('SOR-1-normal', 'SOR-1', 'SOR', '1', 'Standard', 'Normal');",
        )
        .unwrap();

        let static_dir = dir.join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(
            static_dir.join("index.html"),
            "<!doctype html><title>Dumpstar</title>",
        )
        .unwrap();
        shared
    }

    fn router_for(
        dir: &std::path::Path,
        shared: &std::path::Path,
        tenants: Tenants,
        access: Arc<access::AccessState>,
    ) -> Router {
        let registry = {
            let c = dumpstar_db::open_shared(shared).unwrap();
            Arc::new(dumpstar_db::search_meta::load_registry(&c).unwrap())
        };
        let flags = {
            let c = dumpstar_db::open_shared(shared).unwrap();
            Arc::new(dumpstar_db::search_meta::load_flags(&c).unwrap())
        };
        let data_dir = dir.to_path_buf();
        let state = AppState {
            tenants: Arc::new(tenants),
            registry,
            flags,
            data_dir: Arc::new(data_dir.clone()),
            oidc: None,
            access: Some(access),
            mcp: None,
            metrics: metrics::Metrics::disabled(),
        };
        app(state, dir.join("static"), data_dir)
    }

    /// A test router with one tenant, `u`, bound to `u@example.com`.
    async fn test_app() -> (tempfile::TempDir, Router, TestAccessFixture) {
        let (dir, router, _tenants_dir, fx) = multi_tenant_app(&["u"]).await;
        (dir, router, fx)
    }

    /// A test router with `handles` provisioned, each with an
    /// email binding `<handle>@example.com`.
    async fn multi_tenant_app(
        handles: &[&str],
    ) -> (tempfile::TempDir, Router, PathBuf, TestAccessFixture) {
        let dir = tempfile::tempdir().unwrap();
        let shared = seed(dir.path());
        let (router, tenants_dir, fx) = serve_over(dir.path(), &shared, handles).await;
        (dir, router, tenants_dir, fx)
    }

    fn test_tenants(
        tenants_dir: PathBuf,
        shared: PathBuf,
        registry_db: &std::path::Path,
    ) -> Tenants {
        Tenants::new(
            dumpstar_db::app::app().clone(),
            tenants_dir,
            shared,
            registry_db,
            dumpstar_db::user_spec(),
        )
        .unwrap()
    }

    /// The router for a data directory whose catalog already exists at
    /// `shared`, with `handles` provisioned.
    async fn serve_over(
        dir: &std::path::Path,
        shared: &std::path::Path,
        handles: &[&str],
    ) -> (Router, PathBuf, TestAccessFixture) {
        let fx = TestAccessFixture::new().await;
        let tenants_dir = dir.join("tenants");
        std::fs::create_dir_all(&tenants_dir).unwrap();
        let registry_db = dir.join("registry.sqlite");
        let registry = dumpstar_db::open_registry(&registry_db).unwrap();
        for handle in handles {
            let user = dumpstar_db::registry::insert(&registry, handle).unwrap();
            dumpstar_db::open_user(
                &dumpstar_db::tenant_db_file(&tenants_dir, &user.database_id).unwrap(),
            )
            .unwrap();
            dumpstar_db::registry::identity_add(
                &registry,
                &user.database_id,
                &format!("{handle}@example.com"),
                None,
                None,
            )
            .unwrap();
        }
        let router = router_for(
            dir,
            shared,
            test_tenants(tenants_dir.clone(), shared.to_path_buf(), &registry_db),
            fx.access.clone(),
        );
        (router, tenants_dir, fx)
    }

    /// Build a request, optionally injecting a JWT and a body.
    fn request(method: &str, uri: &str, token: Option<&str>, body: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header(access::JWT_HEADER, t);
        }
        match body {
            Some(json) => b
                .header("content-type", "application/json")
                .body(Body::from(json.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        }
    }

    const ADD_CARD: &str = r#"{"printing_id":"SOR-1-normal","source":"manual_id"}"#;

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn health_responds() {
        let (_d, router, _fx) = test_app().await;
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_request_without_token_is_401() {
        let (_d, router, _fx) = test_app().await;
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/collection")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a request with no JWT must be rejected"
        );
    }

    // Seen red first: with backup-status inside the authenticated router, this
    // test fails with 401. Green only once it is in the public router.
    #[tokio::test]
    async fn backup_status_requires_no_token() {
        let (_d, router, _fx) = test_app().await;
        let resp = router
            .oneshot(request("GET", "/api/backup-status", None, None))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/api/backup-status must be reachable without a token (alarm-status.sh reads it from localhost)"
        );
    }

    // Ratchet: every route listed here must still require authentication.
    // If /api/backup-status is the only public route, adding to this list
    // should be a deliberate edit alongside adding to public_api_router().
    #[tokio::test]
    async fn authenticated_api_routes_reject_tokenless_requests() {
        let (_d, router, _fx) = test_app().await;
        let unauthenticated_routes = [
            "GET /api/collection",
            "GET /api/sets",
            "GET /api/card/SOR/1",
            "GET /api/binders",
            "GET /api/decks",
            "GET /api/wishlist",
            "GET /api/conditions",
        ];
        for route in unauthenticated_routes {
            let (method, uri) = route.split_once(' ').unwrap();
            let resp = router
                .clone()
                .oneshot(request(method, uri, None, None))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{route} must require a token; if it is intentionally public, move it to public_api_router()"
            );
        }
    }

    #[tokio::test]
    async fn backup_status_no_marker_is_not_stale() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request("GET", "/api/backup-status", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("\"last_ok_epoch\":null"), "body: {body}");
        assert!(body.contains("\"stale\":false"), "body: {body}");
    }

    #[tokio::test]
    async fn backup_status_old_marker_is_stale() {
        let (dir, router, fx) = test_app().await;
        let old = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 100 * 3600;
        std::fs::write(dir.path().join(".backup-last-ok"), old.to_string()).unwrap();
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request("GET", "/api/backup-status", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("\"stale\":true"), "body: {body}");
    }

    #[tokio::test]
    async fn icons_are_served_as_files() {
        let (d, router, _fx) = test_app().await;
        for (path, mime) in [
            ("/favicon.svg", "image/svg+xml"),
            ("/favicon-32.png", "image/png"),
            ("/apple-touch-icon.png", "image/png"),
        ] {
            std::fs::write(d.path().join("static").join(&path[1..]), b"x").unwrap();
            let r = router
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK, "{path}");
            assert_eq!(r.headers()["content-type"], mime, "{path} is not the file");
        }
    }

    #[tokio::test]
    async fn serves_spa_index_with_fallback() {
        let (_d, router, _fx) = test_app().await;

        let root = router
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root.status(), StatusCode::OK);
        assert!(body_string(root).await.contains("Dumpstar"));

        let spa_route = router
            .oneshot(
                Request::builder()
                    .uri("/collection")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(spa_route.status(), StatusCode::OK);
        assert!(body_string(spa_route).await.contains("Dumpstar"));
    }

    // ---- response compression (pd-2r0p) ----------------------------------

    async fn body_bytes(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    async fn get_encoding(
        router: &Router,
        uri: &str,
        accept: Option<&str>,
        token: Option<&str>,
    ) -> (String, Vec<u8>) {
        let mut b = Request::builder().uri(uri);
        if let Some(a) = accept {
            b = b.header(header::ACCEPT_ENCODING, a);
        }
        if let Some(t) = token {
            b = b.header(access::JWT_HEADER, t);
        }
        let resp = router
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "uri: {uri}");
        let encoding = resp
            .headers()
            .get(header::CONTENT_ENCODING)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        (encoding, body_bytes(resp).await)
    }

    fn gunzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn unbrotli(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        brotli::Decompressor::new(bytes, 4096)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    const BULKY_JSON: &str = "/api/search/keywords";

    #[tokio::test]
    async fn json_gzips_and_decodes_to_the_uncompressed_bytes() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");

        let (plain_encoding, plain) = get_encoding(&router, BULKY_JSON, None, Some(&tok)).await;
        assert_eq!(plain_encoding, "");
        serde_json::from_slice::<serde_json::Value>(&plain).unwrap();
        assert!(
            plain.len() > COMPRESS_MIN_BYTES as usize,
            "fixture too small: {} bytes",
            plain.len()
        );

        let (encoding, compressed) =
            get_encoding(&router, BULKY_JSON, Some("gzip"), Some(&tok)).await;
        assert_eq!(encoding, "gzip");
        assert!(
            compressed.len() < plain.len(),
            "gzip grew the body: {} -> {}",
            plain.len(),
            compressed.len()
        );
        assert_eq!(gunzip(&compressed), plain);
    }

    #[tokio::test]
    async fn brotli_is_used_when_the_client_offers_it() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let (_, plain) = get_encoding(&router, BULKY_JSON, None, Some(&tok)).await;

        let (encoding, compressed) =
            get_encoding(&router, BULKY_JSON, Some("gzip, deflate, br"), Some(&tok)).await;
        assert_eq!(encoding, "br");
        assert_eq!(unbrotli(&compressed), plain);
    }

    #[tokio::test]
    async fn responses_below_the_threshold_are_not_compressed() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let (encoding, body) =
            get_encoding(&router, "/api/backup-status", Some("gzip, br"), Some(&tok)).await;
        assert!(
            body.len() < COMPRESS_MIN_BYTES as usize,
            "no longer a small response: {} bytes",
            body.len()
        );
        assert_eq!(encoding, "");
        serde_json::from_slice::<serde_json::Value>(&body).unwrap();
    }

    #[tokio::test]
    async fn images_are_never_recompressed() {
        let (dir, router, _fx) = test_app().await;
        let symbols = dir.path().join("symbols");
        std::fs::create_dir_all(&symbols).unwrap();
        let png = vec![b'P'; 8 * 1024];
        std::fs::write(symbols.join("SOR.png"), &png).unwrap();

        // Images are not under /api so no token is needed.
        let (encoding, body) = get_encoding(&router, "/sym/SOR.png", Some("gzip, br"), None).await;
        assert_eq!(encoding, "");
        assert_eq!(body, png);
    }

    #[tokio::test]
    async fn set_art_is_served_from_the_data_dir_and_named_in_the_set_list() {
        let (dir, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let listed = |router: Router, tok: String| async move {
            let resp = router
                .oneshot(request("GET", "/api/sets", Some(&tok), None))
                .await
                .unwrap();
            serde_json::from_str::<serde_json::Value>(&body_string(resp).await).unwrap()
        };

        // Nothing retrieved: the tile falls back to the set name.
        let sets = listed(router.clone(), tok.clone()).await;
        assert_eq!(sets[0]["logo_url"], serde_json::Value::Null);
        assert_eq!(sets[0]["art_url"], serde_json::Value::Null);

        // Retrieve a crop the way the catalog job does, then read it back.
        struct One;
        impl dumpstar_catalog::fetch::Fetch for One {
            fn get(&self, _: &str) -> dumpstar_catalog::Result<Vec<u8>> {
                Ok(b"PNG".to_vec())
            }
        }
        let list = dumpstar_catalog::art::sources()
            .into_iter()
            .filter(|s| s.set_code == "SOR")
            .collect::<Vec<_>>();
        dumpstar_catalog::art::fetch(
            &dir.path().join("art"),
            &One,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(),
            &list,
        )
        .unwrap();
        let sets = listed(router.clone(), tok).await;
        assert_eq!(sets[0]["art_url"], "/art/SOR-crop.png");
        let (_, body) = get_encoding(&router, "/art/SOR-crop.png", None, None).await;
        assert_eq!(body, b"PNG");
    }

    #[tokio::test]
    async fn collection_endpoints_round_trip() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");

        let created = router
            .clone()
            .oneshot(request(
                "POST",
                "/api/collection",
                Some(&tok),
                Some(r#"{"printing_id":"SOR-1-normal","source":"manual_id"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        let listed = router
            .clone()
            .oneshot(request("GET", "/api/collection", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert!(body_string(listed).await.contains("SOR-1-normal"));

        let bad = router
            .clone()
            .oneshot(request(
                "POST",
                "/api/collection",
                Some(&tok),
                Some(r#"{"printing_id":"SOR-1-nope","source":"manual_id"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::NOT_FOUND);

        let deleted = router
            .oneshot(request("DELETE", "/api/collection/1", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn manual_price_on_a_catalog_printing_is_a_400_naming_the_seed_file() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");

        let refused = router
            .clone()
            .oneshot(request(
                "POST",
                "/api/manual-prices",
                Some(&tok),
                Some(r#"{"printing_id":"SOR-1-normal","price":29.0}"#),
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert!(
            body_string(refused)
                .await
                .contains("the price of a catalog printing is the catalog's")
        );

        let unknown = router
            .oneshot(request(
                "POST",
                "/api/manual-prices",
                Some(&tok),
                Some(r#"{"printing_id":"nope-0-normal","price":1.0}"#),
            ))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn card_endpoint_returns_detail_and_404() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");

        let found = router
            .clone()
            .oneshot(request("GET", "/api/card/SOR/1", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(found.status(), StatusCode::OK);
        assert!(body_string(found).await.contains("Luke Skywalker"));

        let missing = router
            .oneshot(request("GET", "/api/card/SOR/999", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_owned_and_missing() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");

        router
            .clone()
            .oneshot(request(
                "POST",
                "/api/collection",
                Some(&tok),
                Some(r#"{"printing_id":"SOR-1-normal","source":"manual_id"}"#),
            ))
            .await
            .unwrap();

        let owned = router
            .clone()
            .oneshot(request("GET", "/api/collection/search", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(owned.status(), StatusCode::OK);
        let body = body_string(owned).await;
        assert!(body.contains("SOR-1-normal"), "owned search: {body}");
        assert!(body.contains("\"owned\":true"), "owned flag: {body}");

        let none = router
            .clone()
            .oneshot(request(
                "GET",
                "/api/collection/search?q=t:fire",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(none.status(), StatusCode::OK);
        let body = body_string(none).await;
        assert!(body.contains("\"rows\":[]"), "no matches: {body}");
        assert!(body.contains("\"total\":0"), "no matches: {body}");
    }

    #[tokio::test]
    async fn search_returns_a_page_envelope_with_a_bounded_default() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request(
                "GET",
                "/api/collection/search?include_unowned=1",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(body["rows"].as_array().unwrap().len(), 1);
        assert_eq!(body["total"], 1);
        assert_eq!(body["limit"], dumpstar_db::search::DEFAULT_LIMIT);
        assert_eq!(body["offset"], 0);
    }

    #[tokio::test]
    async fn search_honours_limit_and_offset() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let page = |uri: String| {
            let router = router.clone();
            let tok = tok.clone();
            async move {
                let resp = router
                    .oneshot(request("GET", &uri, Some(&tok), None))
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                serde_json::from_str::<serde_json::Value>(&body_string(resp).await).unwrap()
            }
        };

        let empty = page("/api/collection/search?include_unowned=1&limit=0".to_string()).await;
        assert!(empty["rows"].as_array().unwrap().is_empty());
        assert_eq!(empty["total"], 1, "limit=0 is a count-only request");

        let past_end =
            page("/api/collection/search?include_unowned=1&limit=10&offset=5".to_string()).await;
        assert!(past_end["rows"].as_array().unwrap().is_empty());
        assert_eq!(past_end["total"], 1);
        assert_eq!(past_end["limit"], 10);
        assert_eq!(past_end["offset"], 5);
    }

    #[tokio::test]
    async fn search_cursor_first_page_counts_and_a_bad_cursor_is_a_400() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .clone()
            .oneshot(request(
                "GET",
                "/api/collection/search?include_unowned=1&limit=1&cursor=",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let timing = resp.headers()["server-timing"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(timing.starts_with("totals;"), "{timing}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert!(body["next_cursor"].is_null(), "one row, one page");

        for uri in [
            "/api/collection/search?cursor=zz",
            "/api/collection/search?cursor=&offset=1",
            "/api/collection/search?cursor=&limit=all",
        ] {
            let resp = router
                .clone()
                .oneshot(request("GET", uri, Some(&tok), None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{uri}");
        }
    }

    #[tokio::test]
    async fn search_prices_the_whole_result_and_not_the_page() {
        let (dir, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        router
            .clone()
            .oneshot(request(
                "POST",
                "/api/collection",
                Some(&tok),
                Some(ADD_CARD),
            ))
            .await
            .unwrap();
        {
            let c = dumpstar_db::open_shared(&dir.path().join("shared.sqlite")).unwrap();
            c.execute(
                "INSERT INTO prices (printing_id, source, observed_on, market) \
                 VALUES ('SOR-1-normal', 'tcgcsv', '2025-01-01', 12.5)",
                [],
            )
            .unwrap();
        }

        let page = |uri: String| {
            let router = router.clone();
            let tok = tok.clone();
            async move {
                let resp = router
                    .oneshot(request("GET", &uri, Some(&tok), None))
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                serde_json::from_str::<serde_json::Value>(&body_string(resp).await).unwrap()
            }
        };

        let whole = page("/api/collection/search?limit=all".to_string()).await;
        assert_eq!(whole["total_value"], 12.5);

        let none = page("/api/collection/search?limit=0".to_string()).await;
        assert!(none["rows"].as_array().unwrap().is_empty());
        assert_eq!(none["total_value"], 12.5);
    }

    #[tokio::test]
    async fn search_reports_where_its_time_went_in_server_timing() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request(
                "GET",
                "/api/collection/search?sort=set&limit=all",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let timing = resp.headers()["server-timing"]
            .to_str()
            .unwrap()
            .to_string();
        for stage in ["totals;dur=", "rows;dur=", "copies;dur="] {
            assert!(timing.contains(stage), "{stage} missing from {timing}");
        }
    }

    #[tokio::test]
    async fn search_serves_the_whole_result_for_limit_all() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request(
                "GET",
                "/api/collection/search?include_unowned=1&limit=all",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        let rows = body["rows"].as_array().unwrap().len();
        assert_eq!(body["total"], rows);
        assert_eq!(body["limit"], rows);
        assert_eq!(body["offset"], 0);
    }

    #[tokio::test]
    async fn search_refuses_an_offset_alongside_limit_all() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request(
                "GET",
                "/api/collection/search?limit=all&offset=10",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert!(body["error"].as_str().unwrap().contains("limit=all"));
        assert!(body["position"].is_null());
    }

    #[tokio::test]
    async fn search_refuses_bad_paging_bounds() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let over_max = dumpstar_db::search::MAX_LIMIT + 1;
        for uri in [
            "/api/collection/search?limit=-1".to_string(),
            "/api/collection/search?limit=abc".to_string(),
            "/api/collection/search?limit=1.5".to_string(),
            format!("/api/collection/search?limit={over_max}"),
            "/api/collection/search?limit=99999999999999999999".to_string(),
            "/api/collection/search?offset=-1".to_string(),
            "/api/collection/search?offset=nope".to_string(),
        ] {
            let resp = router
                .clone()
                .oneshot(request("GET", &uri, Some(&tok), None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{uri}");
            let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
            assert!(body["error"].is_string(), "{uri}: {body}");
            assert!(body["position"].is_null(), "not a query error: {uri}");
        }
    }

    #[tokio::test]
    async fn search_refuses_a_sort_it_cannot_satisfy() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        for key in ["value", "adj", "nonsense"] {
            let uri = format!("/api/collection/search?sort={key}");
            let resp = router
                .clone()
                .oneshot(request("GET", &uri, Some(&tok), None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{uri}");
            let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
            let message = body["error"].as_str().unwrap_or_default();
            for named in dumpstar_db::search::SORT_KEYS {
                assert!(message.contains(named), "{uri}: {message} omits {named}");
            }
            assert!(body["position"].is_null(), "not a query error: {uri}");
        }
    }

    #[tokio::test]
    async fn search_serves_every_advertised_sort_key() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        for key in dumpstar_db::search::SORT_KEYS {
            let uri = format!("/api/collection/search?sort={key}&include_unowned=1");
            let resp = router
                .clone()
                .oneshot(request("GET", &uri, Some(&tok), None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        }
    }

    #[tokio::test]
    async fn search_rejects_unknown_keyword_with_position() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request(
                "GET",
                "/api/collection/search?q=xyz:1",
                Some(&tok),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        assert!(body.contains("position"), "expected position in {body}");
        assert!(body.contains("xyz"), "expected keyword in {body}");
    }

    #[tokio::test]
    async fn search_keywords_endpoint_serves_registry() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request("GET", "/api/search/keywords", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("card_type"), "keywords: {body}");
        assert!(body.contains("hyperspace"), "flags: {body}");
    }

    /// sp-r7w5xi acceptance: a catalog written by `catalog-apply --fake` is the
    /// catalog the server serves — through the very open the server makes at
    /// startup — and a search by aspect finds a fake SWU card. The same file,
    /// first written by the fork's legacy card schema,
    /// is rebuilt rather than failed on.
    #[tokio::test]
    async fn a_fake_applied_catalog_is_served_and_searchable_by_aspect() {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let shared = dir.path().join("shared.sqlite");
            if legacy {
                rusqlite::Connection::open(&shared)
                    .unwrap()
                    .execute_batch(dumpstar_catalog::LEGACY_SHAPE_DDL)
                    .unwrap();
            }
            dumpstar_catalog::job::run_once(
                &dumpstar_catalog::FakeSource,
                &dir.path().join("raw"),
                &shared,
                chrono::Utc::now(),
            )
            .unwrap();
            // What `serve` does before it builds its search registry.
            drop(dumpstar_db::open_shared_for_serving(&shared).unwrap());

            let (router, _tenants, fx) = serve_over(dir.path(), &shared, &["u"]).await;
            let tok = fx.valid_token("u@example.com");
            let resp = router
                .oneshot(request(
                    "GET",
                    "/api/collection/search?q=aspect%3Avigilance&include_unowned=1",
                    Some(&tok),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "legacy={legacy}");
            let body = body_string(resp).await;
            assert!(
                body.contains("Luke Skywalker"),
                "legacy={legacy}: search by aspect found nothing: {body}"
            );
            assert!(!body.contains("Millennium Falcon"), "{body}");
        }
    }

    #[tokio::test]
    async fn export_json_serves_a_downloadable_envelope() {
        let (_d, router, fx) = test_app().await;
        let tok = fx.valid_token("u@example.com");
        let resp = router
            .oneshot(request("GET", "/api/export/json", Some(&tok), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .contains("dumpstar-collection.json")
        );
        let envelope: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(
            envelope["format"],
            serde_json::Value::from(dumpstar_db::json_backup::FORMAT)
        );
        assert!(envelope.get("collection").is_some());
        assert!(envelope.get("printings").is_none());
    }

    // ---- tenant resolution and isolation ------------------------------------

    /// The cross-tenant isolation gate (the `isolation` CI tier runs exactly
    /// `tests::isolation`). A test in here is a claim about what one tenant
    /// can never do to another, so it lives where the tier's filter finds it.
    mod isolation {
        use super::*;

        // ---- Pocket ID sign-in (sp-yxan2n) ---------------------------------

        use dumpster_kit_oidc::testing::{FakeProvider, GROUP, TokenSpec, unix_now};
        use serde_json::{Value, json};

        const MCP_RESOURCE: &str = "https://star.test/mcp";

        /// The app over a fake Pocket ID, with `handles` provisioned, and a browser's cookie
        /// jar. No JWKS fixture and no Access: this is the production mode.
        struct Site {
            router: Router,
            provider: FakeProvider,
            tenants_dir: PathBuf,
            jar: Vec<(String, String)>,
            _dir: tempfile::TempDir,
        }

        impl Site {
            async fn new(handles: &[&str]) -> Self {
                Self::with_catalog(handles, "").await
            }

            /// As [`Site::new`], with `extra_sql` run against the seeded catalog first.
            async fn with_catalog(handles: &[&str], extra_sql: &str) -> Self {
                let dir = tempfile::tempdir().unwrap();
                let shared = seed(dir.path());
                if !extra_sql.is_empty() {
                    dumpstar_db::open_shared(&shared)
                        .unwrap()
                        .execute_batch(extra_sql)
                        .unwrap();
                }
                let tenants_dir = dir.path().join("tenants");
                std::fs::create_dir_all(&tenants_dir).unwrap();
                let registry_db = dir.path().join("registry.sqlite");
                let registry = dumpstar_db::open_registry(&registry_db).unwrap();
                for handle in handles {
                    let user = dumpstar_db::registry::insert(&registry, handle).unwrap();
                    dumpstar_db::open_user(
                        &dumpstar_db::tenant_db_file(&tenants_dir, &user.database_id).unwrap(),
                    )
                    .unwrap();
                    dumpstar_db::registry::identity_add(
                        &registry,
                        &user.database_id,
                        &format!("{handle}@example.com"),
                        None,
                        None,
                    )
                    .unwrap();
                }
                let provider = FakeProvider::start().await;
                let mut cfg = provider.config();
                cfg.bypass_paths
                    .extend(OIDC_BYPASS_PATHS.iter().map(|p| p.to_string()));
                cfg.bypass_paths
                    .extend(MCP_BYPASS_PATHS.iter().map(|p| p.to_string()));
                let (issuer, group) = (cfg.issuer.clone(), cfg.required_group.clone());
                let oidc = Oidc::discover(cfg).await.unwrap();
                let c = dumpstar_db::open_shared(&shared).unwrap();
                let tenants = Arc::new(test_tenants(
                    tenants_dir.clone(),
                    shared.clone(),
                    &registry_db,
                ));
                let registry = Arc::new(dumpstar_db::search_meta::load_registry(&c).unwrap());
                let flags = Arc::new(dumpstar_db::search_meta::load_flags(&c).unwrap());
                let telemetry = Telemetry::disabled();
                let auth = McpAuth::discover(
                    McpAuthConfig {
                        issuer,
                        resource: MCP_RESOURCE.into(),
                        required_group: group,
                        clock_skew: std::time::Duration::from_secs(5),
                        jwks_refetch_min: std::time::Duration::ZERO,
                    },
                    tenants.clone(),
                    &telemetry,
                )
                .await
                .unwrap();
                let (tools, resolver) =
                    mcp::tools(tenants.clone(), registry.clone(), flags.clone()).unwrap();
                let state = AppState {
                    tenants,
                    registry,
                    flags,
                    data_dir: Arc::new(dir.path().to_path_buf()),
                    oidc: Some(oidc),
                    access: None,
                    mcp: Some(McpMount {
                        auth,
                        tools,
                        resolver,
                        telemetry,
                    }),
                    metrics: metrics::Metrics::disabled(),
                };
                Site {
                    router: app(state, dir.path().join("static"), dir.path().to_path_buf()),
                    provider,
                    tenants_dir,
                    jar: vec![],
                    _dir: dir,
                }
            }

            async fn send(&mut self, method: &str, uri: &str, body: Option<&str>) -> Response {
                let mut b = Request::builder().method(method).uri(uri);
                if !self.jar.is_empty() {
                    let cookies: Vec<String> =
                        self.jar.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    b = b.header(header::COOKIE, cookies.join("; "));
                }
                let req = match body {
                    Some(json) => b
                        .header("content-type", "application/json")
                        .body(Body::from(json.to_string()))
                        .unwrap(),
                    None => b.body(Body::empty()).unwrap(),
                };
                let resp = self.router.clone().oneshot(req).await.unwrap();
                for sc in resp.headers().get_all(header::SET_COOKIE) {
                    let (nv, _) = sc.to_str().unwrap().split_once(';').unwrap();
                    let (n, v) = nv.split_once('=').unwrap();
                    self.jar.retain(|(k, _)| k != n);
                    if !v.is_empty() {
                        self.jar.push((n.into(), v.into()));
                    }
                }
                resp
            }

            async fn get(&mut self, uri: &str) -> Response {
                self.send("GET", uri, None).await
            }

            /// The whole browser round trip: the login redirect, the provider approving as
            /// `spec`, the callback. Returns the callback's response.
            async fn sign_in(&mut self, spec: TokenSpec) -> Response {
                let r = self.get("/auth/login?return_to=/collection").await;
                assert_eq!(r.status(), StatusCode::SEE_OTHER);
                let loc = r.headers()[header::LOCATION].to_str().unwrap().to_string();
                let state = url::Url::parse(&loc)
                    .unwrap()
                    .query_pairs()
                    .find(|(k, _)| k == "state")
                    .unwrap()
                    .1
                    .into_owned();
                let code = self.provider.authorize(&loc, spec);
                self.get(&format!("/auth/callback?code={code}&state={state}"))
                    .await
            }
        }

        fn member(email: &str) -> TokenSpec {
            TokenSpec {
                email: email.into(),
                ..Default::default()
            }
        }

        /// Acceptance: sign in through the provider and reach the main page and the API as the
        /// tenant the email is bound to.
        #[tokio::test]
        async fn a_member_signs_in_through_the_provider_and_reaches_the_main_page() {
            let mut site = Site::new(&["alice"]).await;

            let r = site.get("/").await;
            assert_eq!(
                r.status(),
                StatusCode::SEE_OTHER,
                "no session: sent to sign in"
            );
            assert!(
                r.headers()[header::LOCATION]
                    .to_str()
                    .unwrap()
                    .starts_with("/auth/login")
            );

            let cb = site.sign_in(member("alice@example.com")).await;
            assert_eq!(cb.status(), StatusCode::SEE_OTHER);
            assert_eq!(cb.headers()[header::LOCATION], "/collection");

            let page = site.get("/").await;
            assert_eq!(page.status(), StatusCode::OK);
            assert!(body_string(page).await.contains("Dumpstar"));

            let created = site.send("POST", "/api/collection", Some(ADD_CARD)).await;
            assert_eq!(created.status(), StatusCode::CREATED);
            let list = site.get("/api/collection").await;
            assert_eq!(list.status(), StatusCode::OK);
            assert!(body_string(list).await.contains("SOR-1-normal"));
        }

        /// Acceptance: a user outside the required group is a 403 and gets no session.
        #[tokio::test]
        async fn a_user_outside_dumpstar_users_gets_403_and_no_session() {
            let mut site = Site::new(&["alice"]).await;
            let outsider = TokenSpec {
                email: "alice@example.com".into(),
                groups: vec!["someone-elses-users".into()],
                ..Default::default()
            };
            assert_ne!(outsider.groups, vec![GROUP.to_string()]);
            let cb = site.sign_in(outsider).await;
            assert_eq!(cb.status(), StatusCode::FORBIDDEN);

            let api = site.get("/api/collection").await;
            assert_eq!(api.status(), StatusCode::UNAUTHORIZED);
            let page = site.get("/").await;
            assert_ne!(page.status(), StatusCode::OK);
        }

        /// A group member whose email no tenant is bound to is a 403 naming the fix, and
        /// nothing is provisioned.
        #[tokio::test]
        async fn a_member_bound_to_no_tenant_is_a_403_and_creates_nothing() {
            let mut site = Site::new(&["alice"]).await;
            let before = std::fs::read_dir(&site.tenants_dir).unwrap().count();
            site.sign_in(member("mallory@example.com")).await;
            let resp = site.get("/api/collection").await;
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
            assert!(
                body_string(resp)
                    .await
                    .contains("dumpstar tenant identity add")
            );
            assert_eq!(
                std::fs::read_dir(&site.tenants_dir).unwrap().count(),
                before
            );
        }

        /// An `/api` call without a session is a 401 — never the redirect a page gets.
        #[tokio::test]
        async fn api_without_a_session_is_401_not_a_redirect() {
            let mut site = Site::new(&["alice"]).await;
            for uri in ["/api/collection", "/api/sets", "/api/binders", "/api/decks"] {
                let r = site.get(uri).await;
                assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{uri}");
            }
            let r = site.send("POST", "/api/collection", Some(ADD_CARD)).await;
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        }

        /// The paths automation reaches without a credential stay open: the liveness probe and
        /// the backup-freshness marker `alarm-status.sh` reads from localhost.
        #[tokio::test]
        async fn the_automation_paths_need_no_session() {
            let mut site = Site::new(&["alice"]).await;
            for uri in ["/health", "/api/backup-status"] {
                let r = site.get(uri).await;
                assert_eq!(r.status(), StatusCode::OK, "{uri}");
            }
        }

        /// "Sign out" ends the app session: the cookie goes, and the API is a 401 again.
        #[tokio::test]
        async fn sign_out_ends_the_session() {
            let mut site = Site::new(&["alice"]).await;
            site.sign_in(member("alice@example.com")).await;
            assert_eq!(site.get("/api/collection").await.status(), StatusCode::OK);

            let out = site.get("/auth/logout").await;
            assert!(out.status().is_redirection());
            assert_eq!(
                site.get("/api/collection").await.status(),
                StatusCode::UNAUTHORIZED
            );
        }

        /// The kit's negative-JWT corpus, minted by the kit's own JWKS fixture, is refused by
        /// dumpstar's router with the status the corpus states; the valid token under the same
        /// fixture is the positive control.
        #[tokio::test]
        async fn every_entry_of_the_kit_negative_jwt_corpus_is_refused() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice"]).await;
            let ok = router
                .clone()
                .oneshot(request(
                    "GET",
                    "/api/collection",
                    Some(&fx.valid_token("alice@example.com")),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(ok.status(), StatusCode::OK, "the positive control");

            for case in dumpster_kit_conformance::negative_jwts() {
                let token = fx
                    .negative_token(&case.name, "alice@example.com")
                    .unwrap_or_else(|| panic!("no minter for corpus entry {}", case.name));
                let resp = router
                    .clone()
                    .oneshot(request("GET", "/api/collection", Some(&token), None))
                    .await
                    .unwrap();
                assert_eq!(
                    resp.status().as_u16(),
                    case.status,
                    "{}: {}",
                    case.name,
                    case.describes
                );
            }
        }

        /// Each tenant's JWT email resolves to their own database only.
        #[tokio::test]
        async fn one_tenant_cannot_reach_another_tenants_collection() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let alice_tok = fx.valid_token("alice@example.com");
            let bob_tok = fx.valid_token("bob@example.com");

            let created = router
                .clone()
                .oneshot(request(
                    "POST",
                    "/api/collection",
                    Some(&alice_tok),
                    Some(ADD_CARD),
                ))
                .await
                .unwrap();
            assert_eq!(created.status(), StatusCode::CREATED);

            let bobs = router
                .clone()
                .oneshot(request("GET", "/api/collection", Some(&bob_tok), None))
                .await
                .unwrap();
            assert_eq!(bobs.status(), StatusCode::OK);
            let body = body_string(bobs).await;
            assert_eq!(
                body.trim(),
                "[]",
                "bob's collection must not contain alice's card: {body}"
            );

            let stolen = router
                .clone()
                .oneshot(request("DELETE", "/api/collection/1", Some(&bob_tok), None))
                .await
                .unwrap();
            assert_eq!(
                stolen.status(),
                StatusCode::NOT_FOUND,
                "bob deleted a row out of alice's collection"
            );

            let alices = router
                .oneshot(request("GET", "/api/collection", Some(&alice_tok), None))
                .await
                .unwrap();
            assert!(body_string(alices).await.contains("SOR-1-normal"));
        }

        /// The deck-line routes act on the signed-in tenant's decks only, and a
        /// line the tenant cannot fill is refused with the card named (sp-mmdoxy).
        #[tokio::test]
        async fn deck_lines_are_per_tenant_and_over_reservation_is_refused() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let alice_tok = fx.valid_token("alice@example.com");
            let bob_tok = fx.valid_token("bob@example.com");
            let send = |method: &'static str, uri: &'static str, tok: &str, body: Option<&str>| {
                let router = router.clone();
                let req = request(method, uri, Some(tok), body);
                async move {
                    let resp = router.oneshot(req).await.unwrap();
                    (resp.status(), body_string(resp).await)
                }
            };
            let (s, _) = send(
                "POST",
                "/api/decks",
                &alice_tok,
                Some(r#"{"name":"Pack 1","format":"Jumpstart","group":"cube"}"#),
            )
            .await;
            assert_eq!(s, StatusCode::CREATED);

            let lines = r#"{"lines":[{"card_id":"NOPE","zone":"main","qty":2}]}"#;
            let (s, body) = send("PUT", "/api/decks/1/lines", &alice_tok, Some(lines)).await;
            assert_eq!(s, StatusCode::CONFLICT);
            assert!(
                body.contains("NOPE") && body.contains("\"needed\":2"),
                "{body}"
            );

            for (m, u) in [
                ("GET", "/api/decks/1/lines"),
                ("POST", "/api/decks/1/release"),
                ("GET", "/api/decks/1/validate"),
            ] {
                let (s, _) = send(m, u, &bob_tok, None).await;
                assert_eq!(
                    s,
                    StatusCode::NOT_FOUND,
                    "bob reached alice's deck: {m} {u}"
                );
            }
            let (s, _) = send(
                "PUT",
                "/api/decks/1/lines",
                &bob_tok,
                Some(r#"{"lines":[]}"#),
            )
            .await;
            assert_eq!(s, StatusCode::NOT_FOUND);

            let (s, body) = send("GET", "/api/card-counts", &alice_tok, None).await;
            assert_eq!(s, StatusCode::OK, "{body}");
        }

        /// An unbound email is a 403 and does not create a database.
        #[tokio::test]
        async fn an_unbound_email_is_a_403_and_creates_nothing() {
            let (_d, router, tenants_dir, fx) = multi_tenant_app(&["alice"]).await;
            let tok = fx.valid_token("mallory@example.com");
            let before = std::fs::read_dir(&tenants_dir).unwrap().count();
            let resp = router
                .oneshot(request("GET", "/api/collection", Some(&tok), None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
            assert_eq!(std::fs::read_dir(&tenants_dir).unwrap().count(), before);
        }

        /// The acceptance list at the HTTP boundary: a bound email is a 200, an
        /// unbound one a 403 naming the fix, and no token, a garbage token and
        /// every entry of the negative-JWT list a 401 — with nothing provisioned
        /// by any of them.
        #[tokio::test]
        async fn the_access_gate_at_the_router() {
            let (_d, router, tenants_dir, fx) = multi_tenant_app(&["alice"]).await;
            let databases = || {
                std::fs::read_dir(&tenants_dir)
                    .unwrap()
                    .filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .filter(|n| n.ends_with(".sqlite"))
                    .collect::<Vec<_>>()
            };
            let before = databases();
            let status = |tok: Option<String>| {
                let router = router.clone();
                async move {
                    let resp = router
                        .oneshot(request("GET", "/api/collection", tok.as_deref(), None))
                        .await
                        .unwrap();
                    (resp.status(), body_string(resp).await)
                }
            };

            let (s, _) = status(Some(fx.valid_token("alice@example.com"))).await;
            assert_eq!(s, StatusCode::OK);

            let (s, body) = status(Some(fx.valid_token("mallory@example.com"))).await;
            assert_eq!(s, StatusCode::FORBIDDEN);
            assert!(
                body.contains("dumpstar tenant identity add")
                    && body.contains("mallory@example.com"),
                "the 403 must name the fix: {body}"
            );

            let e = "alice@example.com";
            let negatives = [
                ("no token", None),
                ("garbage", Some("not.a.jwt".to_string())),
                ("expired", Some(fx.expired_token(e))),
                ("nbf", Some(fx.not_yet_valid_token(e))),
                ("wrong aud", Some(fx.wrong_aud_token(e))),
                ("wrong iss", Some(fx.wrong_issuer_token(e))),
                ("unknown kid", Some(fx.unknown_kid_token(e))),
                ("tampered", Some(fx.tampered_token(e))),
                ("alg=none", Some(fx.alg_none_token(e))),
                (
                    "HS256 with the public key",
                    Some(fx.alg_hs256_pubkey_token(e)),
                ),
            ];
            for (name, tok) in negatives {
                let (s, body) = status(tok).await;
                assert_eq!(
                    s,
                    StatusCode::UNAUTHORIZED,
                    "{name} was not rejected: {body}"
                );
            }
            assert_eq!(databases(), before, "a request provisioned a database");
        }

        /// Detach takes access away; reattach gives it back, with the data.
        #[tokio::test]
        async fn reattach_restores_access() {
            let (d, router, _tenants_dir, fx) = multi_tenant_app(&["alice"]).await;
            let tok = fx.valid_token("alice@example.com");
            let get = || {
                router
                    .clone()
                    .oneshot(request("GET", "/api/collection", Some(&tok), None))
            };
            let created = router
                .clone()
                .oneshot(request(
                    "POST",
                    "/api/collection",
                    Some(&tok),
                    Some(ADD_CARD),
                ))
                .await
                .unwrap();
            assert_eq!(created.status(), StatusCode::CREATED);

            let reg = dumpstar_db::open_registry(&d.path().join("registry.sqlite")).unwrap();
            let alice = dumpstar_db::registry::detach(&reg, "alice").unwrap();
            assert_eq!(get().await.unwrap().status(), StatusCode::FORBIDDEN);

            dumpstar_db::registry::reattach(&reg, &alice.database_id).unwrap();
            let resp = get().await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(body_string(resp).await.contains("SOR-1-normal"));
        }

        /// A leftover `x-dumpstar-tenant` header cannot be used to reach another
        /// tenant's collection. The header is not read; resolution is from the JWT
        /// email only. Sending bob's handle alongside alice's JWT still gives alice.
        #[tokio::test]
        async fn leftover_tenant_header_cannot_escalate_to_another_tenant() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let alice_tok = fx.valid_token("alice@example.com");

            // Alice adds a card.
            let created = router
                .clone()
                .oneshot(request(
                    "POST",
                    "/api/collection",
                    Some(&alice_tok),
                    Some(ADD_CARD),
                ))
                .await
                .unwrap();
            assert_eq!(created.status(), StatusCode::CREATED);

            // Now alice sends a request with her own JWT but a header claiming to
            // be bob. Resolution ignores the header; she still reaches alice's DB.
            let resp = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/api/collection")
                        .header(access::JWT_HEADER, &alice_tok)
                        .header("x-dumpstar-tenant", "bob")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_string(resp).await;
            assert!(
                body.contains("SOR-1-normal"),
                "alice's header claim resolved to the wrong collection: {body}"
            );
        }

        #[tokio::test]
        async fn there_is_no_database_outside_a_resolved_request() {
            let (dir, _router, tenants_dir, fx) = multi_tenant_app(&["u"]).await;
            let shared = dir.path().join("shared.sqlite");
            let registry_db = dir.path().join("registry.sqlite");
            let database_id = {
                let reg = dumpstar_db::open_registry(&registry_db).unwrap();
                dumpstar_db::registry::lookup(&reg, "u")
                    .unwrap()
                    .unwrap()
                    .database_id
            };
            let state = AppState {
                tenants: Arc::new(test_tenants(tenants_dir, shared, &registry_db)),
                registry: Arc::new(Default::default()),
                flags: Arc::new(Vec::new()),
                data_dir: Arc::new(dir.path().to_path_buf()),
                oidc: None,
                access: Some(fx.access.clone()),
                mcp: None,
                metrics: metrics::Metrics::disabled(),
            };

            let unscoped = blocking(&state, |c| {
                Ok(c.query_row("SELECT count(*) FROM collection", [], |r| {
                    r.get::<_, i64>(0)
                })?)
            })
            .await;
            assert!(unscoped.is_err(), "a connection without a resolved tenant");

            let scoped = tenant::as_tenant(
                &database_id,
                blocking(&state, |c| {
                    Ok(c.query_row("SELECT count(*) FROM collection", [], |r| {
                        r.get::<_, i64>(0)
                    })?)
                }),
            )
            .await;
            assert_eq!(scoped.unwrap(), 0);
        }

        /// Tenant A creates a row in every tenant-owned resource the API
        /// exposes by id; tenant B, holding a perfectly valid JWT of their
        /// own, can neither read, change nor delete any of them, and A's rows
        /// are untouched afterwards. Row ids are per-database counters, so B
        /// naming id 1 is exactly the guess an attacker would make.
        #[tokio::test]
        async fn tenant_b_cannot_read_change_or_delete_tenant_a_rows() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let a = fx.valid_token("alice@example.com");
            let b = fx.valid_token("bob@example.com");
            let send = |method: &'static str, uri: &'static str, tok: &str, body: Option<&str>| {
                let router = router.clone();
                let req = request(method, uri, Some(tok), body);
                async move {
                    let resp = router.oneshot(req).await.unwrap();
                    (resp.status(), body_string(resp).await)
                }
            };

            for (uri, body) in [
                ("/api/collection", ADD_CARD),
                ("/api/wishlist", r#"{"card_id":"SOR-1"}"#),
                ("/api/binders", r#"{"name":"alice-binder"}"#),
                ("/api/decks", r#"{"name":"alice-deck"}"#),
            ] {
                let (s, out) = send("POST", uri, &a, Some(body)).await;
                assert_eq!(s, StatusCode::CREATED, "alice could not seed {uri}: {out}");
            }

            // B sees none of it in any listing...
            for uri in [
                "/api/collection",
                "/api/wishlist",
                "/api/binders",
                "/api/decks",
            ] {
                let (s, out) = send("GET", uri, &b, None).await;
                assert_eq!(s, StatusCode::OK, "{uri}");
                assert_eq!(out.trim(), "[]", "bob's {uri} lists alice's rows: {out}");
            }

            // ...cannot fetch, edit or delete a row by id...
            for (method, uri, body) in [
                ("GET", "/api/collection/1", None),
                ("PUT", "/api/collection/1", Some(r#"{"notes":"pwned"}"#)),
                ("DELETE", "/api/collection/1", None),
                ("PUT", "/api/wishlist/1", Some(r#"{"notes":"pwned"}"#)),
                ("DELETE", "/api/wishlist/1", None),
                ("GET", "/api/binders/1", None),
                ("PUT", "/api/binders/1", Some(r#"{"name":"pwned"}"#)),
                ("DELETE", "/api/binders/1", None),
                ("GET", "/api/decks/1", None),
                ("PUT", "/api/decks/1", Some(r#"{"name":"pwned"}"#)),
                ("DELETE", "/api/decks/1", None),
            ] {
                let (s, out) = send(method, uri, &b, body).await;
                assert_eq!(
                    s,
                    StatusCode::NOT_FOUND,
                    "bob reached alice's row with {method} {uri}: {out}"
                );
            }

            // ...and everything is still there, unchanged, for A.
            for (uri, needle) in [
                ("/api/collection", "SOR-1-normal"),
                ("/api/wishlist", "SOR-1"),
                ("/api/binders", "alice-binder"),
                ("/api/decks", "alice-deck"),
            ] {
                let (s, out) = send("GET", uri, &a, None).await;
                assert_eq!(s, StatusCode::OK, "{uri}");
                assert!(
                    out.contains(needle),
                    "alice lost {needle} from {uri}: {out}"
                );
                assert!(!out.contains("pwned"), "bob's write reached {uri}: {out}");
            }
        }

        /// Moving a copy between binders is visible in both binders' views at
        /// once, and a move into a binder that exists only in another tenant's
        /// database is rejected with the copy left where it was.
        #[tokio::test]
        async fn moving_a_copy_between_binders_updates_both_views_and_stops_at_the_tenant() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let a = fx.valid_token("alice@example.com");
            let b = fx.valid_token("bob@example.com");
            let send = |method: &'static str, uri: &'static str, tok: &str, body: Option<&str>| {
                let router = router.clone();
                let req = request(method, uri, Some(tok), body);
                async move {
                    let resp = router.oneshot(req).await.unwrap();
                    (resp.status(), body_string(resp).await)
                }
            };
            let json = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();

            for body in [
                r#"{"name":"one","pocket_size":9}"#,
                r#"{"name":"two","pocket_size":12}"#,
            ] {
                let (s, out) = send("POST", "/api/binders", &a, Some(body)).await;
                assert_eq!(s, StatusCode::CREATED, "{out}");
            }
            let (s, out) = send("POST", "/api/collection", &a, Some(ADD_CARD)).await;
            assert_eq!(s, StatusCode::CREATED, "{out}");

            let in_binder = |out: String| json(&out)["cards"].as_array().unwrap().len();
            let count_of = |out: String, name: &str| {
                json(&out)
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|x| x["name"] == name)
                    .unwrap()["card_count"]
                    .as_i64()
                    .unwrap()
            };

            let (s, out) = send(
                "PUT",
                "/api/collection/1/move",
                &a,
                Some(r#"{"binder_id":1}"#),
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{out}");
            let (_, one) = send("GET", "/api/binders/1", &a, None).await;
            assert_eq!(in_binder(one), 1);

            let (s, out) = send(
                "PUT",
                "/api/collection/1/move",
                &a,
                Some(r#"{"binder_id":2}"#),
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{out}");
            let (_, one) = send("GET", "/api/binders/1", &a, None).await;
            let (_, two) = send("GET", "/api/binders/2", &a, None).await;
            assert_eq!(in_binder(one), 0, "old binder still lists the copy");
            assert_eq!(in_binder(two), 1, "new binder does not list the copy");
            let (_, list) = send("GET", "/api/binders", &a, None).await;
            assert_eq!(count_of(list.clone(), "one"), 0);
            assert_eq!(count_of(list, "two"), 1);

            // Bob owns a copy and no binders: alice's binder ids mean nothing to him.
            let (s, out) = send("POST", "/api/collection", &b, Some(ADD_CARD)).await;
            assert_eq!(s, StatusCode::CREATED, "{out}");
            let (s, out) = send(
                "PUT",
                "/api/collection/1/move",
                &b,
                Some(r#"{"binder_id":2}"#),
            )
            .await;
            assert_eq!(
                s,
                StatusCode::NOT_FOUND,
                "cross-tenant move accepted: {out}"
            );
            let (s, out) = send("GET", "/api/collection/1", &b, None).await;
            assert_eq!(s, StatusCode::OK, "{out}");
            assert!(json(&out)["binder_id"].is_null(), "bob's copy moved: {out}");
            let (_, two) = send("GET", "/api/binders/2", &a, None).await;
            assert_eq!(in_binder(two), 1, "bob's move reached alice's binder");
        }

        /// Tenant A's copy, named by id through every collection route that
        /// takes one (and the ones that do not): tenant B changes nothing and
        /// reads nothing, and A's copy is intact afterwards.
        #[tokio::test]
        async fn tenant_b_cannot_reach_tenant_a_copies_through_any_collection_route() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let a = fx.valid_token("alice@example.com");
            let b = fx.valid_token("bob@example.com");
            let send = |method: &'static str, uri: &'static str, tok: &str, body: Option<&str>| {
                let router = router.clone();
                let req = request(method, uri, Some(tok), body);
                async move {
                    let resp = router.oneshot(req).await.unwrap();
                    (resp.status(), body_string(resp).await)
                }
            };
            let (s, out) = send("POST", "/api/collection", &a, Some(ADD_CARD)).await;
            assert_eq!(s, StatusCode::CREATED, "{out}");

            for (method, uri, body) in [
                ("GET", "/api/collection/1", None),
                ("PUT", "/api/collection/1", Some(r#"{"notes":"pwned"}"#)),
                (
                    "PUT",
                    "/api/collection/1/status",
                    Some(r#"{"status":"sold"}"#),
                ),
                ("PUT", "/api/collection/1/move", Some(r#"{"binder_id":1}"#)),
                (
                    "PUT",
                    "/api/collection/1/printing",
                    Some(r#"{"printing_id":"SOR-1-normal"}"#),
                ),
                ("DELETE", "/api/collection/by-printing/SOR-1-normal", None),
                ("DELETE", "/api/collection/1", None),
            ] {
                let (s, out) = send(method, uri, &b, body).await;
                assert_eq!(
                    s,
                    StatusCode::NOT_FOUND,
                    "bob reached alice's copy with {method} {uri}: {out}"
                );
            }

            let (s, out) = send("POST", "/api/collection/bulk-delete", &b, Some("[1]")).await;
            assert_eq!(s, StatusCode::OK, "{out}");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&out).unwrap()["deleted"],
                0,
                "bob's bulk delete removed alice's copy: {out}"
            );

            for uri in [
                "/api/collection/search?include_unowned=1&limit=all",
                "/api/collection/value-history",
                "/api/export/json",
            ] {
                let (s, out) = send("GET", uri, &b, None).await;
                assert_eq!(s, StatusCode::OK, "{uri}: {out}");
                if uri.contains("search") {
                    assert!(
                        !out.contains("\"owned\":true"),
                        "bob's search shows alice's copy as owned: {out}"
                    );
                } else {
                    assert!(
                        !out.contains("SOR-1-normal"),
                        "bob reads alice's copy through {uri}: {out}"
                    );
                }
            }

            let (s, out) = send("GET", "/api/collection/1", &a, None).await;
            assert_eq!(s, StatusCode::OK, "alice lost her copy: {out}");
            assert!(out.contains("SOR-1-normal"), "{out}");
            assert!(!out.contains("pwned") && !out.contains("sold"), "{out}");
        }

        /// Browse: every catalogued set has a page, and the owned overlay on the
        /// sets index, the set page and the card detail is the asking tenant's
        /// own collection and nobody else's.
        #[tokio::test]
        async fn browse_serves_every_set_and_counts_only_the_tenants_own_copies() {
            let (d, router, _dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            {
                let c = dumpstar_db::open_shared(&d.path().join("shared.sqlite")).unwrap();
                c.execute_batch(
                    "INSERT INTO sets (set_code, name) VALUES ('empty', 'Empty'); \
                     INSERT INTO sets (set_code, name) VALUES ('two', 'Two'); \
                     INSERT INTO cards (card_id, name) VALUES ('two-1', 'A'), ('two-2', 'B'); \
                     INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                       VALUES ('two-1-normal', 'two-1', 'two', '1', 'Standard', 'Normal'), \
                              ('two-1-foil', 'two-1', 'two', '1', 'Standard', 'Foil'), \
                              ('two-2-normal', 'two-2', 'two', '2', 'Standard', 'Normal');",
                )
                .unwrap();
            }
            let a = fx.valid_token("alice@example.com");
            let b = fx.valid_token("bob@example.com");
            let get = |tok: &str, uri: String| {
                let router = router.clone();
                let req = request("GET", &uri, Some(tok), None);
                async move {
                    let resp = router.oneshot(req).await.unwrap();
                    let status = resp.status();
                    let body = body_string(resp).await;
                    (
                        status,
                        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                    )
                }
            };
            for pid in ["two-1-normal", "two-1-foil", "two-2-normal"] {
                let resp = router
                    .clone()
                    .oneshot(request(
                        "POST",
                        "/api/collection",
                        Some(&a),
                        Some(&format!(
                            r#"{{"printing_id":"{pid}","source":"manual_id"}}"#
                        )),
                    ))
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::CREATED, "{pid}");
            }

            for (tok, who, want_cards, want_copies) in [(&a, "alice", 2, 3), (&b, "bob", 0, 0)] {
                let (s, sets) = get(tok, "/api/sets".into()).await;
                assert_eq!(s, StatusCode::OK);
                let rows = sets.as_array().unwrap();
                for code in ["SOR", "empty", "two"] {
                    let row = rows.iter().find(|r| r["set_code"] == code);
                    let row = row.unwrap_or_else(|| panic!("{who}: set {code} missing"));
                    let (s, page) = get(tok, format!("/api/sets/{code}/binder")).await;
                    assert_eq!(s, StatusCode::OK, "{who}: set {code} has no page");
                    if code == "two" {
                        assert_eq!(row["owned_cards"], want_cards, "{who} index");
                        let owned: i64 = page["slots"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .flat_map(|s| s["printings"].as_array().unwrap())
                            .map(|p| p["owned_count"].as_i64().unwrap())
                            .sum();
                        assert_eq!(owned, want_copies, "{who} set page overlay");
                    } else {
                        assert_eq!(row["owned_cards"], 0, "{who} {code}");
                    }
                }
                let (s, card) = get(tok, "/api/card/two/1".into()).await;
                assert_eq!(s, StatusCode::OK);
                let printings = card["printings"].as_array().unwrap();
                assert_eq!(printings.len(), 2, "card detail lists every printing");
                let owned: i64 = printings
                    .iter()
                    .map(|p| p["owned_count"].as_i64().unwrap())
                    .sum();
                assert_eq!(
                    owned,
                    if who == "alice" { 2 } else { 0 },
                    "{who} card detail"
                );
            }
        }

        /// A 100,000-copy collection lists in one gzip response that decodes to
        /// every row, and the same route for another tenant stays empty.
        #[tokio::test]
        async fn a_hundred_thousand_copies_list_gzipped_and_stay_private() {
            const N: usize = 100_000;
            let (_d, router, tenants_dir, fx) = multi_tenant_app(&["alice", "bob"]).await;
            let a = fx.valid_token("alice@example.com");
            let b = fx.valid_token("bob@example.com");
            let database_id = {
                let reg = dumpstar_db::open_registry(&_d.path().join("registry.sqlite")).unwrap();
                dumpstar_db::registry::lookup(&reg, "alice")
                    .unwrap()
                    .unwrap()
                    .database_id
            };
            {
                let conn = dumpstar_db::open_user(
                    &dumpstar_db::tenant_db_file(&tenants_dir, &database_id).unwrap(),
                )
                .unwrap();
                conn.execute_batch(&format!(
                    "INSERT INTO collection (printing_id, acquired_at, source)
                     WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {N})
                     SELECT 'SOR-1-normal', '2026-01-01', 'manual_id' FROM n;"
                ))
                .unwrap();
            }

            let (encoding, compressed) =
                get_encoding(&router, "/api/collection", Some("gzip"), Some(&a)).await;
            assert_eq!(encoding, "gzip");
            let plain = gunzip(&compressed);
            assert!(
                compressed.len() * 5 < plain.len(),
                "gzip barely helped: {} -> {}",
                plain.len(),
                compressed.len()
            );
            let rows: Vec<serde_json::Value> = serde_json::from_slice(&plain).unwrap();
            assert_eq!(rows.len(), N);

            let (_, bobs) = get_encoding(&router, "/api/collection", Some("gzip"), Some(&b)).await;
            assert_eq!(String::from_utf8(bobs).unwrap().trim(), "[]");
        }

        /// Routes that are deliberately reachable without a credential. Adding
        /// to this list is the deliberate edit the ratchet below demands.
        const PUBLIC_API_ROUTES: &[&str] = &["/backup-status"];

        /// Every route literal registered in `src/routes/*.rs`, found by
        /// reading the source — so a route added tomorrow is in the set without
        /// anyone remembering to list it.
        fn registered_route_paths() -> Vec<String> {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
            let mut paths = Vec::new();
            for entry in std::fs::read_dir(dir).unwrap() {
                let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
                let production = match text.find("#[cfg(test)]") {
                    Some(i) => &text[..i],
                    None => &text,
                };
                let mut rest = production;
                while let Some(i) = rest.find(".route(") {
                    rest = rest[i + ".route(".len()..].trim_start();
                    if let Some(lit) = rest.strip_prefix('"') {
                        let end = lit.find('"').unwrap();
                        paths.push(lit[..end].to_string());
                    }
                }
            }
            paths.sort();
            paths.dedup();
            paths
        }

        /// A request carrying no credential at all, to `uri` with `method`.
        async fn tokenless(router: &Router, method: &str, uri: &str) -> Response {
            router
                .clone()
                .oneshot(request(method, uri, None, None))
                .await
                .unwrap()
        }

        /// The ratchet over authenticated routes. Every route the source
        /// registers must answer a tokenless request with a 401 on at least one
        /// method and never with anything but a 401, a 405 or the SPA shell on
        /// the others — unless it is in [`PUBLIC_API_ROUTES`]. A route added
        /// to either router without the access layer in front of it fails here.
        #[tokio::test]
        async fn every_registered_route_requires_a_token_unless_listed_public() {
            let (_d, router, _fx) = test_app().await;
            let paths = registered_route_paths();
            assert!(
                paths.len() > 40,
                "the route scan found {} routes; it has stopped reading the source",
                paths.len()
            );
            for p in PUBLIC_API_ROUTES {
                assert!(
                    paths.iter().any(|q| q == p),
                    "{p} is listed public but no longer registered; delete it from the list"
                );
            }

            for path in paths {
                if PUBLIC_API_ROUTES.contains(&path.as_str()) {
                    continue;
                }
                let concrete = path.replace("{printing_id}", "x").replace(['{', '}'], "");
                let concrete = concrete
                    .split('/')
                    .map(|seg| {
                        if seg == "id" || seg == "set" || seg == "number" || seg == "code" {
                            "1"
                        } else {
                            seg
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                // A route is mounted either at `/api` or, for the collection
                // module, under `/api/collection`.
                let mut rejected = false;
                for prefix in ["", "/collection"] {
                    let tail = if concrete == "/" {
                        ""
                    } else {
                        concrete.as_str()
                    };
                    let uri = format!("/api{prefix}{tail}");
                    for method in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
                        let resp = tokenless(&router, method, &uri).await;
                        let status = resp.status();
                        let is_shell = status == StatusCode::OK
                            && resp
                                .headers()
                                .get(header::CONTENT_TYPE)
                                .is_some_and(|v| v.to_str().unwrap().starts_with("text/html"));
                        match status {
                            StatusCode::UNAUTHORIZED => rejected = true,
                            StatusCode::METHOD_NOT_ALLOWED => {}
                            _ if is_shell => {}
                            _ => panic!(
                                "{method} {uri} answered {status} to a request with no token; \
                                 put the route behind the access layer, or list it in \
                                 PUBLIC_API_ROUTES if it is meant to be public"
                            ),
                        }
                    }
                }
                assert!(
                    rejected,
                    "route {path} never answered 401; the ratchet cannot see it"
                );
            }
        }

        /// The public list is also true in the other direction: a listed
        /// public route really is reachable with no credential.
        #[tokio::test]
        async fn listed_public_routes_need_no_token() {
            let (_d, router, _fx) = test_app().await;
            for p in PUBLIC_API_ROUTES {
                let resp = tokenless(&router, "GET", &format!("/api{p}")).await;
                assert_eq!(resp.status(), StatusCode::OK, "{p}");
            }
        }

        const PRIVATE_NO_STORE: &str = "private, no-store";

        fn assert_private_no_store(what: &str, resp: &Response) {
            let values: Vec<_> = resp
                .headers()
                .get_all(header::CACHE_CONTROL)
                .iter()
                .map(|v| v.to_str().unwrap().to_string())
                .collect();
            assert_eq!(
                values,
                vec![PRIVATE_NO_STORE.to_string()],
                "{what} ({}) must carry exactly `Cache-Control: {PRIVATE_NO_STORE}`",
                resp.status()
            );
        }

        /// Every `/api` response — success, client error, auth failure, an
        /// unmatched path, the public route — is private and no-store, so no
        /// shared cache between the origin and a tenant's browser can hold one
        /// tenant's body and hand it to another.
        #[tokio::test]
        async fn every_api_response_is_private_no_store() {
            let (_d, router, _dir, fx) = multi_tenant_app(&["alice"]).await;
            let alice = fx.valid_token("alice@example.com");
            let mallory = fx.valid_token("mallory@example.com");
            let cases = vec![
                (
                    "200 json",
                    "GET",
                    "/api/collection",
                    Some(alice.clone()),
                    None,
                ),
                (
                    "201 created",
                    "POST",
                    "/api/collection",
                    Some(alice.clone()),
                    Some(ADD_CARD),
                ),
                (
                    "404 by id",
                    "GET",
                    "/api/collection/999",
                    Some(alice.clone()),
                    None,
                ),
                (
                    "400 bad query",
                    "GET",
                    "/api/collection/search?q=%28",
                    Some(alice.clone()),
                    None,
                ),
                (
                    "export download",
                    "GET",
                    "/api/export/json",
                    Some(alice.clone()),
                    None,
                ),
                ("403 unbound", "GET", "/api/collection", Some(mallory), None),
                ("401 no token", "GET", "/api/collection", None, None),
                (
                    "401 garbage token",
                    "GET",
                    "/api/collection",
                    Some("not.a.jwt".into()),
                    None,
                ),
                ("public route", "GET", "/api/backup-status", None, None),
                (
                    "unmatched /api path",
                    "GET",
                    "/api/no-such-route",
                    None,
                    None,
                ),
            ];
            for (what, method, uri, tok, body) in cases {
                let resp = router
                    .clone()
                    .oneshot(request(method, uri, tok.as_deref(), body))
                    .await
                    .unwrap();
                assert_private_no_store(what, &resp);
            }

            // Outside /api nothing changes: the SPA shell is not tenant data.
            let shell = tokenless(&router, "GET", "/").await;
            assert!(shell.headers().get(header::CACHE_CONTROL).is_none());
        }

        // ---- Agent tools at /mcp (sp-cuhb3r) -------------------------------

        impl Site {
            fn token_for(&self, email: &str) -> String {
                self.provider.mint_jwt(
                    json!({
                        "iss": self.provider.issuer, "aud": MCP_RESOURCE,
                        "sub": format!("sub-{email}"), "email": email, "groups": [GROUP],
                        "client_id": "claude-code", "exp": unix_now() + 3600,
                    }),
                    None,
                )
            }

            async fn rpc(&self, token: &str, method: &str, params: Value) -> Value {
                let req = Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                            .to_string(),
                    ))
                    .unwrap();
                let resp = self.router.clone().oneshot(req).await.unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                serde_json::from_str(&body_string(resp).await).unwrap()
            }

            /// A tool's structured result, or its error text.
            async fn tool(&self, token: &str, name: &str, args: Value) -> Result<Value, String> {
                let r = self
                    .rpc(
                        token,
                        "tools/call",
                        json!({"name": name, "arguments": args}),
                    )
                    .await;
                let r = &r["result"];
                if r["isError"] == true {
                    Err(r["content"][0]["text"].as_str().unwrap().to_string())
                } else {
                    Ok(r["structuredContent"].clone())
                }
            }
        }

        const SECOND_CARD: &str = "INSERT INTO cards (card_id, name, card_type, cost, aspects) \
               VALUES ('SOR-2', 'Han Solo', 'Unit', 4, '[\"Cunning\",\"Heroism\"]'); \
             INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
               VALUES ('SOR-2-normal', 'SOR-2', 'SOR', '2', 'Standard', 'Normal');";

        /// `/mcp` without a bearer is a 401 pointing at the metadata, and the metadata is
        /// served without a session; a browser session cookie does not open it.
        #[tokio::test]
        async fn mcp_without_a_bearer_is_401_and_a_cookie_is_not_one() {
            let mut site = Site::new(&["alice"]).await;
            let r = site.get("/mcp").await;
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
            assert!(
                r.headers()[header::WWW_AUTHENTICATE]
                    .to_str()
                    .unwrap()
                    .contains("resource_metadata=")
            );
            let meta = site.get("/.well-known/oauth-protected-resource/mcp").await;
            assert_eq!(meta.status(), StatusCode::OK);
            let v: Value = serde_json::from_str(&body_string(meta).await).unwrap();
            assert_eq!(v["resource"], MCP_RESOURCE);

            site.sign_in(member("alice@example.com")).await;
            assert_eq!(site.get("/mcp").await.status(), StatusCode::UNAUTHORIZED);
        }

        /// The card object carries the catalog's keywords, uniqueness and epic action.
        #[tokio::test]
        async fn the_mcp_card_object_carries_keywords_unique_and_epic_action() {
            let site = Site::with_catalog(
                &["alice"],
                &format!("{SECOND_CARD} UPDATE cards SET keywords='[{{\"name\":\"Raid\",\"value\":2}}]', is_unique=1, \
                   epic_action='Deal 3 damage to a unit.', \
                   rules_text='Raid 2\nShielded\nEpic Action: Deal 3 damage to a unit.' WHERE card_id='SOR-2'; \
                 INSERT INTO cards (card_id, name, card_type) VALUES ('SOR-9', 'Plain', 'Event'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('SOR-9-normal', 'SOR-9', 'SOR', '9', 'Standard', 'Normal');"),
            )
            .await;
            let token = site.token_for("alice@example.com");
            let full = site
                .tool(&token, "get_card", json!({"set": "SOR", "number": "9"}))
                .await
                .unwrap();
            let g = &full["card"]["game"];
            assert_eq!(g["keywords"], json!([]));
            assert_eq!(g["unique"], json!(false));
            assert!(g["epic_action"].is_null());

            let han = site
                .tool(&token, "get_card", json!({"id": "SOR-2"}))
                .await
                .unwrap();
            let g = &han["card"]["game"];
            assert_eq!(g["keywords"], json!([{"name": "Raid", "value": 2}]));
            assert_eq!(g["unique"], json!(true));
            assert_eq!(g["epic_action"], json!("Deal 3 damage to a unit."));

            let found = site
                .tool(&token, "search_cards", json!({"q": "han"}))
                .await
                .unwrap();
            assert_eq!(found["results"][0]["game"]["unique"], json!(true));
            let text = "Raid 2\nShielded\nEpic Action: Deal 3 damage to a unit.";
            assert_eq!(found["results"][0]["game"]["rules_text"], json!(text));
            assert_eq!(
                found["results"][0]["game"]["epic_action"],
                json!("Deal 3 damage to a unit.")
            );
            assert_eq!(
                found["results"][0]["game"]["keywords"],
                json!([{"name": "Raid", "value": 2}])
            );
            assert_eq!(han["card"]["game"]["rules_text"], json!(text));
            for q in ["k:raid", "has:epic", "is:unique"] {
                let r = site
                    .tool(&token, "search_cards", json!({"q": q}))
                    .await
                    .unwrap();
                assert_eq!(r["total"], json!(1), "{q}: {r}");
            }
        }

        /// sp-sk3oz4: every card id the tools return is `<SET>-<number>`, whatever the catalog's
        /// own key is, and the catalog's older forms (`n:<slug>`, bare numerics) still go in.
        #[tokio::test]
        async fn mcp_card_ids_are_one_documented_form_and_old_forms_are_accepted() {
            let site = Site::with_catalog(
                &["alice"],
                "INSERT OR IGNORE INTO sets (set_code, name) VALUES ('JTL', 'Jump to Lightspeed'); \
                 INSERT INTO cards (card_id, name, card_type) VALUES ('n:vanquish', 'Vanquish', 'Event'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('p-van-h', 'n:vanquish', 'JTL', '300', 'Hyperspace', 'Normal'), \
                          ('p-van', 'n:vanquish', 'JTL', '045', 'Standard', 'Normal'); \
                 INSERT INTO cards (card_id, name, card_type) VALUES ('6472095064', 'Krennic', 'Unit'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('p-kre', '6472095064', 'SOR', '12', 'Standard', 'Normal');",
            )
            .await;
            let token = site.token_for("alice@example.com");
            let found = site
                .tool(&token, "search_cards", json!({"q": "", "limit": 100}))
                .await
                .unwrap();
            let shape = |id: &str| {
                id.split_once('-').is_some_and(|(set, n)| {
                    !set.is_empty()
                        && set
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                        && !n.is_empty()
                        && n.chars().all(|c| c.is_ascii_alphanumeric())
                })
            };
            let rows = found["results"].as_array().unwrap();
            assert!(rows.len() >= 3);
            for r in rows {
                let id = r["id"].as_str().unwrap();
                assert!(shape(id), "id {id} is not <SET>-<number>");
            }
            let van: Vec<_> = rows.iter().filter(|r| r["name"] == "Vanquish").collect();
            assert!(van.iter().all(|r| r["id"] == "JTL-045"), "one id per card");

            for given in ["n:vanquish", "JTL-045", "jtl-45", "JTL-300"] {
                let got = site
                    .tool(&token, "get_card", json!({"id": given}))
                    .await
                    .unwrap();
                assert_eq!(got["card"]["id"], "JTL-045", "{given}");
            }
            let deck = site
                .tool(&token, "create_deck", json!({"name": "d", "format": "x"}))
                .await
                .unwrap()["deck"]["id"]
                .as_i64()
                .unwrap();
            let short = site
                .tool(
                    &token,
                    "set_deck_lines",
                    json!({"id": deck, "lines": [{"card_id": "6472095064", "qty": 1}]}),
                )
                .await;
            let err = short.unwrap_err();
            assert!(err.contains("SOR-12"), "{err}");
        }

        /// A bearer token opens nothing under `/api`.
        #[tokio::test]
        async fn a_bearer_token_does_not_open_the_api() {
            let site = Site::new(&["alice"]).await;
            let token = site.token_for("alice@example.com");
            let req = Request::builder()
                .uri("/api/collection")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let resp = site.router.clone().oneshot(req).await.unwrap();
            assert_ne!(resp.status(), StatusCode::OK);
        }

        /// The whole v1 contract as one tenant, and a second tenant sees none of it.
        #[tokio::test]
        async fn an_agent_builds_a_deck_from_owned_copies_and_no_copy_lands_twice() {
            let mut site = Site::with_catalog(&["alice", "bob"], SECOND_CARD).await;
            site.sign_in(member("alice@example.com")).await;
            for _ in 0..2 {
                let r = site.send("POST", "/api/collection", Some(ADD_CARD)).await;
                assert_eq!(r.status(), StatusCode::CREATED);
            }
            let alice = site.token_for("alice@example.com");
            let bob = site.token_for("bob@example.com");

            let listed = site.rpc(&alice, "tools/list", json!({})).await;
            let names: Vec<&str> = listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect();
            for want in dumpster_kit_mcp::v1_tools() {
                assert!(names.contains(&want.as_str()), "missing {want}");
            }

            let found = site
                .tool(
                    &alice,
                    "search_cards",
                    json!({"q": "luke", "owned_only": true}),
                )
                .await
                .unwrap();
            assert_eq!(found["results"][0]["id"], "SOR-1");
            assert_eq!(found["results"][0]["owned_count"], 2);
            assert_eq!(found["results"][0]["available_count"], 2);
            assert_eq!(found["results"][0]["game"]["aspects"], json!(["Vigilance"]));

            let a = site
                .tool(
                    &alice,
                    "create_deck",
                    json!({"name": "Pack 1", "format": "Jumpstart", "group": "Cube"}),
                )
                .await
                .unwrap()["deck"]["id"]
                .as_i64()
                .unwrap();
            let b = site
                .tool(
                    &alice,
                    "create_deck",
                    json!({"name": "Pack 2", "format": "Jumpstart", "group": "Cube"}),
                )
                .await
                .unwrap()["deck"]["id"]
                .as_i64()
                .unwrap();
            let one = json!({"lines": [{"card_id": "SOR-1", "qty": 2}]});
            let set = site
                .tool(
                    &alice,
                    "set_deck_lines",
                    json!({"id": a, "lines": one["lines"]}),
                )
                .await
                .unwrap();
            assert_eq!(set["reserved"], 2);

            let again = site
                .tool(
                    &alice,
                    "set_deck_lines",
                    json!({"id": b, "lines": one["lines"]}),
                )
                .await;
            let refusal = again.unwrap_err();
            assert!(refusal.contains("not enough free copies"));
            assert!(
                refusal.contains("Pack 1"),
                "refusal names the holder: {refusal}"
            );
            assert!(
                refusal.contains("(id ")
                    && refusal.contains("qty ")
                    && refusal.contains("set_deck_lines"),
                "refusal gives a next step: {refusal}"
            );
            let found = site
                .tool(&alice, "search_cards", json!({"q": "", "owned_only": true}))
                .await
                .unwrap();
            let row = found["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == "SOR-1")
                .expect("SOR-1 in results");
            assert_eq!(
                row["in_decks"],
                json!([{"id": a, "name": "Pack 1", "qty": 2}])
            );
            let got = site
                .tool(&alice, "get_card", json!({"id": "SOR-1"}))
                .await
                .unwrap();
            assert_eq!(
                got["card"]["in_decks"],
                json!([{"id": a, "name": "Pack 1", "qty": 2}])
            );
            let deck = site
                .tool(&alice, "get_deck", json!({"id": a}))
                .await
                .unwrap();
            let in_deck = deck["cards"]
                .as_object()
                .unwrap()
                .values()
                .find(|c| c["id"] == "SOR-1")
                .expect("SOR-1 in get_deck cards");
            assert_eq!(in_deck["in_decks"], row["in_decks"]);
            let summary = site
                .tool(&alice, "collection_summary", json!({}))
                .await
                .unwrap();
            assert_eq!(summary["reserved_copies"], 2);
            assert_eq!(summary["available_copies"], 0);

            let freed = site
                .tool(&alice, "release_deck", json!({"id": a}))
                .await
                .unwrap();
            assert_eq!(freed["released"], 2);
            site.tool(
                &alice,
                "set_deck_lines",
                json!({"id": b, "lines": one["lines"]}),
            )
            .await
            .unwrap();
            let unsanctioned = site
                .tool(&alice, "validate_deck", json!({"id": b}))
                .await
                .unwrap();
            assert_eq!(unsanctioned["sanctioned"], false);

            let theirs = site.tool(&bob, "list_decks", json!({})).await.unwrap();
            assert_eq!(theirs["decks"], json!([]));
            assert!(site.tool(&bob, "get_deck", json!({"id": b})).await.is_err());
            let none = site
                .tool(&bob, "search_cards", json!({"q": "", "owned_only": true}))
                .await
                .unwrap();
            assert_eq!(none["results"], json!([]));
            site.tool(&alice, "delete_deck", json!({"id": b}))
                .await
                .unwrap();
        }

        /// sp-4be2yr: every resource an agent can create (a `create_<r>` tool) has get, list,
        /// update and delete tools too, so no resource needs delete-and-recreate to change.
        #[tokio::test]
        async fn every_agent_writable_resource_has_create_get_list_update_delete() {
            let site = Site::new(&["alice"]).await;
            let t = site.token_for("alice@example.com");
            let listed = site.rpc(&t, "tools/list", json!({})).await;
            let names: Vec<String> = listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect();
            let resources: Vec<&str> = names
                .iter()
                .filter_map(|n| n.strip_prefix("create_"))
                .collect();
            assert!(resources.contains(&"deck"), "decks are agent-writable");
            for r in resources {
                for want in [
                    format!("get_{r}"),
                    format!("list_{r}s"),
                    format!("update_{r}"),
                    format!("delete_{r}"),
                ] {
                    assert!(
                        names.contains(&want),
                        "{r} is writable but {want} is missing"
                    );
                }
            }
        }

        async fn get_deck(site: &Site, tok: &str, id: i64) -> Value {
            site.tool(tok, "get_deck", json!({"id": id})).await.unwrap()["deck"].clone()
        }

        /// sp-4be2yr: update_deck is a partial update that keeps the id.
        #[tokio::test]
        async fn update_deck_renames_regroups_and_clears_keeping_the_id() {
            let mut site = Site::with_catalog(&["alice", "bob"], SECOND_CARD).await;
            site.sign_in(member("alice@example.com")).await;
            let alice = site.token_for("alice@example.com");
            let bob = site.token_for("bob@example.com");
            let id = site
                .tool(
                    &alice,
                    "create_deck",
                    json!({"name": "Pack 1", "format": "Jumpstart", "group": "Cube A", "description": "d", "notes": "n"}),
                )
                .await
                .unwrap()["deck"]["id"]
                .as_i64()
                .unwrap();

            let r = site
                .tool(&alice, "update_deck", json!({"id": id, "name": "Pack One"}))
                .await
                .unwrap();
            assert_eq!(r["deck"]["id"], id);
            let d = get_deck(&site, &alice, id).await;
            assert_eq!(d["name"], "Pack One");
            assert_eq!(d["group"], "Cube A", "absent key unchanged");
            assert_eq!(d["description"], "d");

            site.tool(&alice, "update_deck", json!({"id": id, "group": "Cube B"}))
                .await
                .unwrap();
            let d = get_deck(&site, &alice, id).await;
            assert_eq!(
                (d["group"].as_str(), d["name"].as_str()),
                (Some("Cube B"), Some("Pack One"))
            );

            site.tool(
                &alice,
                "update_deck",
                json!({"id": id, "description": null}),
            )
            .await
            .unwrap();
            let d = get_deck(&site, &alice, id).await;
            assert!(d["description"].is_null());
            assert_eq!(d["notes"], "n");
            assert_eq!(d["id"], id);

            let e = site
                .tool(&alice, "update_deck", json!({"id": 9999, "name": "x"}))
                .await;
            assert!(e.unwrap_err().contains("not found"));
            let e = site
                .tool(&bob, "update_deck", json!({"id": id, "name": "x"}))
                .await;
            assert!(e.unwrap_err().contains("not found"));
            assert_eq!(get_deck(&site, &alice, id).await["name"], "Pack One");
            let e = site
                .tool(&alice, "update_deck", json!({"id": id, "colour": "red"}))
                .await;
            assert!(e.unwrap_err().contains("unknown key 'colour'"));
            let e = site
                .tool(&alice, "update_deck", json!({"id": id, "name": null}))
                .await;
            assert!(e.is_err());
        }

        /// sp-8y7ro4: the skill's second search (`a:<color> -a:heroism -a:villainy -t:base`) finds
        /// the same-color card with no alignment that the aligned search cannot.
        #[tokio::test]
        async fn the_skills_unaligned_search_returns_a_same_color_card() {
            let sql = "INSERT INTO cards (card_id, name, card_type, cost, arenas, aspects) VALUES \
                 ('V1', 'Aligned Vigilance', 'Event', 2, '[]', '[\"Vigilance\",\"Heroism\"]'), \
                 ('V2', 'Vanquish', 'Event', 5, '[]', '[\"Vigilance\"]'), \
                 ('V3', 'Off Color Pair', 'Unit', 3, '[]', '[\"Vigilance\",\"Cunning\"]'), \
                 ('V4', 'Off Color Aligned', 'Unit', 3, '[]', '[\"Vigilance\",\"Command\",\"Heroism\"]'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) VALUES \
                 ('V1-p', 'V1', 'SOR', 'V1', 'Standard', 'Normal'), \
                 ('V2-p', 'V2', 'SOR', 'V2', 'Standard', 'Normal'), \
                 ('V3-p', 'V3', 'SOR', 'V3', 'Standard', 'Normal'), \
                 ('V4-p', 'V4', 'SOR', 'V4', 'Standard', 'Normal');";
            let mut site = Site::with_catalog(&["alice"], sql).await;
            site.sign_in(member("alice@example.com")).await;
            for p in ["V1-p", "V2-p", "V3-p", "V4-p"] {
                let body = json!({"printing_id": p, "source": "manual_id"}).to_string();
                let r = site.send("POST", "/api/collection", Some(&body)).await;
                assert_eq!(r.status(), StatusCode::CREATED);
            }
            let t = site.token_for("alice@example.com");
            let ids = |r: &serde_json::Value| -> Vec<String> {
                r["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["id"].as_str().unwrap().to_string())
                    .collect()
            };
            let aligned = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": "a:vigilance a:heroism -a:command -a:aggression -a:cunning", "owned_only": true, "limit": 100}),
                )
                .await
                .unwrap();
            assert_eq!(ids(&aligned), vec!["SOR-V1"]);
            let unaligned = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": "a:vigilance -a:heroism -a:villainy -a:command -a:aggression -a:cunning -t:base", "owned_only": true, "limit": 100}),
                )
                .await
                .unwrap();
            assert_eq!(ids(&unaligned), vec!["SOR-V2"]);
        }

        /// sp-70k718: the skill carries no collection state, and its documented pool queries,
        /// run over a fixture catalog, return neither Leaders nor bases.
        #[tokio::test]
        async fn the_skill_states_no_collection_fact_and_its_pool_queries_return_no_leaders() {
            let skill = std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../.claude/skills/swu-jumpstart-cube/SKILL.md"
            ))
            .unwrap();
            for claim in ["owns only", "owns just", "only owns", "you own only"] {
                assert!(
                    !skill.to_lowercase().contains(claim),
                    "SKILL.md states an owned-count fact ({claim:?}); derive it from collection_summary"
                );
            }
            for line in skill.lines() {
                for q in line.split('`').skip(1).step_by(2) {
                    if ["t:", "a:"].iter().any(|p| q.starts_with(p)) {
                        assert!(
                            q.contains("avail>=1"),
                            "documented query lacks avail>=1: {q}"
                        );
                    }
                }
            }
            let template = |label: &str| -> String {
                let line = skill
                    .lines()
                    .find(|l| l.trim_start().starts_with(label))
                    .unwrap_or_else(|| panic!("SKILL.md has no {label} query"));
                let q = line.split('`').nth(1).unwrap().to_string();
                q.replace(" avail>=1", "")
            };
            let fill = |q: String| -> String {
                q.replace("<color>", "vigilance")
                    .replace("<alignment>", "heroism")
                    .replace(
                        "-a:<each other color>",
                        "-a:command -a:aggression -a:cunning",
                    )
                    .replace(
                        "-a:<every color>",
                        "-a:command -a:vigilance -a:aggression -a:cunning",
                    )
            };
            let aligned = fill(template("- color+alignment:"));
            let unaligned = fill(template("- color only:"));
            let alignment_only = fill(template("- alignment only:"));
            let no_aspect = fill(template("- no aspect:"));
            for q in [&aligned, &unaligned, &alignment_only, &no_aspect] {
                assert!(!q.contains('<'), "{q}");
            }

            let sql = "INSERT INTO cards (card_id, name, card_type, cost, arenas, aspects) VALUES \
                 ('P1', 'Pool Unit', 'Unit', 2, '[]', '[\"Vigilance\",\"Heroism\"]'), \
                 ('P2', 'Pool Leader', 'Leader', 5, '[]', '[\"Vigilance\",\"Heroism\"]'), \
                 ('P3', 'Pool Base', 'Base', 0, '[]', '[\"Vigilance\"]'), \
                 ('P4', 'Unaligned Leader', 'Leader', 5, '[]', '[\"Vigilance\"]'), \
                 ('P5', 'Vanquish', 'Event', 5, '[]', '[\"Vigilance\"]'), \
                 ('P6', 'Alliance X-Wing', 'Unit', 3, '[]', '[\"Heroism\"]'), \
                 ('P7', 'AT-ST', 'Unit', 6, '[]', '[\"Villainy\"]'), \
                 ('P8', 'Blank Event', 'Event', 2, '[]', '[]'), \
                 ('P9', 'Off Color Heroism', 'Unit', 3, '[]', '[\"Command\",\"Heroism\"]'), \
                 ('P10', 'No Aspect Leader', 'Leader', 5, '[]', '[]'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) VALUES \
                 ('P1-p', 'P1', 'SOR', 'P1', 'Standard', 'Normal'), \
                 ('P2-p', 'P2', 'SOR', 'P2', 'Standard', 'Normal'), \
                 ('P3-p', 'P3', 'SOR', 'P3', 'Standard', 'Normal'), \
                 ('P4-p', 'P4', 'SOR', 'P4', 'Standard', 'Normal'), \
                 ('P5-p', 'P5', 'SOR', 'P5', 'Standard', 'Normal'), \
                 ('P6-p', 'P6', 'SOR', 'P6', 'Standard', 'Normal'), \
                 ('P7-p', 'P7', 'SOR', 'P7', 'Standard', 'Normal'), \
                 ('P8-p', 'P8', 'SOR', 'P8', 'Standard', 'Normal'), \
                 ('P9-p', 'P9', 'SOR', 'P9', 'Standard', 'Normal'), \
                 ('P10-p', 'P10', 'SOR', 'P10', 'Standard', 'Normal');";
            let mut site = Site::with_catalog(&["alice"], sql).await;
            site.sign_in(member("alice@example.com")).await;
            for p in [
                "P1-p", "P2-p", "P3-p", "P4-p", "P5-p", "P6-p", "P7-p", "P8-p", "P9-p", "P10-p",
            ] {
                let body = json!({"printing_id": p, "source": "manual_id"}).to_string();
                let r = site.send("POST", "/api/collection", Some(&body)).await;
                assert_eq!(r.status(), StatusCode::CREATED);
            }
            let t = site.token_for("alice@example.com");
            let ids = |r: &serde_json::Value| -> Vec<String> {
                r["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["id"].as_str().unwrap().to_string())
                    .collect()
            };
            let a = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": aligned, "owned_only": true, "limit": 100}),
                )
                .await
                .unwrap();
            assert_eq!(ids(&a), vec!["SOR-P1"]);
            let u = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": unaligned, "owned_only": true, "limit": 100}),
                )
                .await
                .unwrap();
            assert_eq!(ids(&u), vec!["SOR-P5"]);
            for (q, want) in [(alignment_only, "SOR-P6"), (no_aspect, "SOR-P8")] {
                let r = site
                    .tool(
                        &t,
                        "search_cards",
                        json!({"q": q, "owned_only": true, "limit": 100}),
                    )
                    .await
                    .unwrap();
                assert_eq!(ids(&r), vec![want], "{q}");
            }
        }

        /// Acceptance (sp-kk6hn8): the swu-jumpstart-cube skill's method, scripted with no model,
        /// through the `/mcp` tools over a fixture collection saves 20 decks that pass `check_cube`.
        #[tokio::test]
        async fn the_jumpstart_cube_method_saves_20_packs_that_pass_check_cube() {
            use dumpstar_cube::fixture::SKELETON;

            // (color, heroism packs); the other packs of the color are villainy: 10 and 10.
            let plan = [
                ("Command", 3),
                ("Vigilance", 2),
                ("Aggression", 3),
                ("Cunning", 2),
            ];
            let epic = [
                "Energy Conversion Lab",
                "Security Complex",
                "Tarkintown",
                "Jedha City",
            ];
            let mut packs: Vec<(usize, &str, &str)> = vec![];
            for (color, heroes) in plan {
                for k in 0..5 {
                    let a = if k < heroes { "Heroism" } else { "Villainy" };
                    packs.push((packs.len() + 1, color, a));
                }
            }

            // The fixture collection: a leader and 15 skeleton cards per pack, and five copies of
            // each color's two bases.
            let mut sql = String::new();
            let mut owned: Vec<(String, u32)> = vec![];
            let mut add = |id: &str,
                           name: &str,
                           kind: &str,
                           cost: u32,
                           arena: &str,
                           hp: Option<i64>,
                           aspects: &str,
                           copies: u32| {
                let arenas = if arena.is_empty() {
                    "[]".to_string()
                } else {
                    format!("[\"{arena}\"]")
                };
                let hp = hp.map_or("NULL".to_string(), |h| h.to_string());
                let epic_action = if hp == "25" {
                    "'Epic Action: Draw a card.'"
                } else {
                    "NULL"
                };
                sql.push_str(&format!(
                    "INSERT INTO cards (card_id, name, card_type, cost, hp, arenas, aspects, epic_action) \
                       VALUES ('{id}', '{name}', '{kind}', {cost}, {hp}, '{arenas}', '{aspects}', {epic_action}); \
                     INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                       VALUES ('{id}-p', '{id}', 'SOR', '{id}', 'Standard', 'Normal'); "
                ));
                owned.push((format!("{id}-p"), copies));
            };
            for (n, color, align) in &packs {
                let aspects = format!("[\"{color}\",\"{align}\"]");
                add(
                    &format!("L{n}"),
                    &format!("Leader {n}"),
                    "Leader",
                    5,
                    "Ground",
                    Some(6),
                    &aspects,
                    1,
                );
                for (i, (kind, arena, cost)) in SKELETON.iter().enumerate() {
                    add(
                        &format!("C{n}x{i}"),
                        &format!("Card {n} {i}"),
                        kind,
                        *cost,
                        arena,
                        None,
                        &aspects,
                        1,
                    );
                }
            }
            for (i, (color, _)) in plan.iter().enumerate() {
                let one = format!("[\"{color}\"]");
                add(
                    &format!("B25{color}"),
                    epic[i],
                    "Base",
                    0,
                    "",
                    Some(25),
                    &one,
                    5,
                );
                add(
                    &format!("B30{color}"),
                    &format!("{color} Vanilla Base"),
                    "Base",
                    0,
                    "",
                    Some(30),
                    &one,
                    5,
                );
            }

            let mut site = Site::with_catalog(&["alice"], &sql).await;
            site.sign_in(member("alice@example.com")).await;
            for (printing, copies) in &owned {
                for _ in 0..*copies {
                    let body = json!({"printing_id": printing, "source": "manual_id"}).to_string();
                    let r = site.send("POST", "/api/collection", Some(&body)).await;
                    assert_eq!(r.status(), StatusCode::CREATED);
                }
            }
            let t = site.token_for("alice@example.com");

            // Step 1: collection_summary.
            let summary = site
                .tool(&t, "collection_summary", json!({}))
                .await
                .unwrap();
            assert_eq!(summary["reserved_copies"], 0);

            // Steps 2-5: for each pack, fill the skeleton from what is still available, save it,
            // read it back.
            let mut docs = vec![];
            for (n, color, align) in &packs {
                let (c, a) = (color.to_lowercase(), align.to_lowercase());
                let mut cards = serde_json::Map::new();
                let mut lines = vec![];
                // Two searches a pack keep the whole run inside the endpoint's 120 calls a minute:
                // the pool of the leader's two aspects, and the color's bases.
                let pool = site
                    .tool(
                        &t,
                        "search_cards",
                        json!({"q": format!("a:{c} a:{a}"), "owned_only": true, "limit": 100}),
                    )
                    .await
                    .unwrap();
                let bases = site
                    .tool(
                        &t,
                        "search_cards",
                        json!({"q": format!("t:base a:{c} -t:leader avail>=1"), "owned_only": true}),
                    )
                    .await
                    .unwrap();
                let mut taken: Vec<String> = vec![];
                type Want = (String, Option<i64>, Option<u64>, Option<String>);
                let mut wants: Vec<Want> = vec![
                    ("Leader".into(), None, None, None),
                    ("Base".into(), Some(25), None, None),
                    ("Base".into(), Some(30), None, None),
                ];
                for (kind, arena, cost) in SKELETON {
                    wants.push((
                        kind.into(),
                        None,
                        Some(u64::from(cost)),
                        (!arena.is_empty()).then(|| arena.to_string()),
                    ));
                }
                for (kind, hp, cost, arena) in wants {
                    let from = if kind == "Base" { &bases } else { &pool };
                    let card = from["results"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| {
                            r["type"] == kind.as_str()
                                && r["available_count"].as_u64().unwrap() >= 1
                                && !taken.contains(&r["id"].as_str().unwrap().to_string())
                                && hp.is_none_or(|h| r["game"]["hp"] == h)
                                && cost.is_none_or(|k| r["cost"] == k)
                                && arena
                                    .as_ref()
                                    .is_none_or(|ar| r["game"]["arenas"][0] == ar.as_str())
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "pack {n}: nothing available for {kind} {hp:?} {cost:?} {arena:?}"
                            )
                        })
                        .clone();
                    let id = card["id"].as_str().unwrap().to_string();
                    taken.push(id.clone());
                    lines.push(json!({"card_id": id, "qty": 1}));
                    cards.insert(id, card);
                }
                let deck = site
                    .tool(
                        &t,
                        "create_deck",
                        json!({
                            "name": format!("Pack {n}"), "format": "SWU Jumpstart Cube",
                            "group": "swu-jumpstart-cube",
                        }),
                    )
                    .await
                    .unwrap()["deck"]["id"]
                    .as_i64()
                    .unwrap();
                site.tool(&t, "set_deck_lines", json!({"id": deck, "lines": lines}))
                    .await
                    .unwrap();
                let got = site
                    .tool(&t, "get_deck", json!({"id": deck}))
                    .await
                    .unwrap();
                for (id, card) in &cards {
                    assert_eq!(got["cards"][id]["id"], card["id"], "pack {n}: {id}");
                    assert_eq!(got["cards"][id]["type"], card["type"]);
                }
                assert_eq!(
                    got["lines_digest"].as_str().map(str::len),
                    Some(64),
                    "pack {n}: get_deck carries a SHA-256 lines_digest"
                );
                dumpstar_cube::verify_digest(&got).unwrap();
                let doc = got.clone();
                let failures = dumpstar_cube::check_pack_value(&doc).unwrap();
                assert!(failures.is_empty(), "pack {n}: {failures:?}");
                docs.push(doc);
            }

            let cube = dumpstar_cube::check_cube_value(&Value::Array(docs)).unwrap();
            assert!(cube.is_empty(), "{cube:?}");
            let decks = site.tool(&t, "list_decks", json!({})).await.unwrap();
            assert_eq!(decks["decks"].as_array().unwrap().len(), 20);
            let after = site
                .tool(&t, "collection_summary", json!({}))
                .await
                .unwrap();
            assert_eq!(after["reserved_copies"], 20 * 18);
        }

        /// sp-jceiw2: collection_summary breaks owned and available copies down by type,
        /// aspect and set, bases by HP and epic/ability/vanilla, leaders by aspect and alignment.
        #[tokio::test]
        async fn collection_summary_breaks_the_collection_down() {
            let mut sql = String::from(
                "INSERT INTO sets (set_code, name, release_date, sort_order) \
                   VALUES ('JTL', 'Jump to Lightspeed', '2025-03-14', 2); ",
            );
            let mut owned: Vec<(&str, u32)> = vec![];
            for (id, set, kind, hp, aspects, epic, copies) in [
                (
                    "T-L1",
                    "SOR",
                    "Leader",
                    None,
                    r#"["Vigilance","Heroism"]"#,
                    false,
                    1,
                ),
                (
                    "T-L2",
                    "SOR",
                    "Leader",
                    None,
                    r#"["Aggression","Villainy"]"#,
                    false,
                    2,
                ),
                ("T-U1", "JTL", "Unit", None, r#"["Vigilance"]"#, false, 3),
                ("T-B1", "SOR", "Base", Some(25), r#"["Command"]"#, true, 2),
                (
                    "T-B2",
                    "JTL",
                    "Base",
                    Some(30),
                    r#"["Vigilance"]"#,
                    false,
                    1,
                ),
                ("T-B3", "SOR", "Base", Some(28), r#"["Command"]"#, false, 1),
            ] {
                let hp = hp.map_or("NULL".to_string(), |h: i64| h.to_string());
                let epic = if epic {
                    "'Epic Action: Draw a card.'"
                } else {
                    "NULL"
                };
                sql.push_str(&format!(
                    "INSERT INTO cards (card_id, name, card_type, hp, aspects, epic_action) \
                       VALUES ('{id}', '{id}', '{kind}', {hp}, '{aspects}', {epic}); \
                     INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                       VALUES ('{id}-p', '{id}', '{set}', '{id}', 'Standard', 'Normal'); "
                ));
                owned.push((Box::leak(format!("{id}-p").into_boxed_str()), copies));
            }
            sql.push_str(
                "UPDATE cards SET rules_text = 'When played, heal 2.' WHERE card_id = 'T-B3';",
            );
            let mut site = Site::with_catalog(&["alice"], &sql).await;
            site.sign_in(member("alice@example.com")).await;
            for (printing, copies) in &owned {
                for _ in 0..*copies {
                    let body = json!({"printing_id": printing, "source": "manual_id"}).to_string();
                    let r = site.send("POST", "/api/collection", Some(&body)).await;
                    assert_eq!(r.status(), StatusCode::CREATED);
                }
            }
            let t = site.token_for("alice@example.com");
            let deck = site
                .tool(
                    &t,
                    "create_deck",
                    json!({"name": "One", "format": "Jumpstart", "group": "Cube"}),
                )
                .await
                .unwrap()["deck"]["id"]
                .as_i64()
                .unwrap();
            site.tool(
                &t,
                "set_deck_lines",
                json!({"id": deck, "lines": [{"card_id": "T-U1", "qty": 1}]}),
            )
            .await
            .unwrap();

            let b = site
                .tool(&t, "collection_summary", json!({}))
                .await
                .unwrap()["breakdown"]
                .clone();
            let oa = |o: u64, a: u64| json!({"owned": o, "available": a});
            let od = |d: u64, o: u64, a: u64| json!({"distinct": d, "owned": o, "available": a});
            assert_eq!(
                b["by_type"],
                json!({"Base": oa(4, 4), "Leader": oa(3, 3), "Unit": oa(3, 2)})
            );
            assert_eq!(
                b["by_aspect"],
                json!({
                    "Aggression": oa(2, 2), "Command": oa(3, 3), "Heroism": oa(1, 1),
                    "Vigilance": oa(5, 4), "Villainy": oa(2, 2),
                })
            );
            assert_eq!(b["by_set"], json!({"JTL": oa(4, 3), "SOR": oa(6, 6)}));
            assert_eq!(
                b["bases"]["by_hp"],
                json!({"25": oa(2, 2), "28": oa(1, 1), "30": oa(1, 1)})
            );
            assert_eq!(
                b["bases"]["by_kind"],
                json!({"ability": oa(1, 1), "epic": oa(2, 2), "vanilla": oa(1, 1)})
            );
            assert_eq!(
                b["bases"]["by_kind_aspect"],
                json!({
                    "ability": {"Command": oa(1, 1)},
                    "epic": {"Command": oa(2, 2)},
                    "vanilla": {"Vigilance": oa(1, 1)},
                })
            );
            assert_eq!(
                b["leaders"]["by_aspect"],
                json!({
                    "Aggression": od(1, 2, 2), "Heroism": od(1, 1, 1),
                    "Vigilance": od(1, 1, 1), "Villainy": od(1, 2, 2),
                })
            );
            assert_eq!(
                b["leaders"]["by_alignment"],
                json!({"Heroism": od(1, 1, 1), "Villainy": od(1, 2, 2)})
            );

            // sp-l7yh3y: the copy totals describe the whole result, not the page: six cards,
            // ten owned copies, nine not in a deck, however small the page.
            let first = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": "", "owned_only": true, "limit": 2}),
                )
                .await
                .unwrap();
            assert_eq!(first["results"].as_array().unwrap().len(), 2);
            assert_eq!(first["total"], 6);
            assert_eq!(first["total_copies_owned"], 10);
            assert_eq!(first["total_copies_available"], 9);
            assert_eq!(first["total_copies_reserved"], 1);
            let second = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": "", "owned_only": true, "limit": 2, "cursor": first["next_cursor"]}),
                )
                .await
                .unwrap();
            assert_eq!(second["total_copies_owned"], 10);
            assert_eq!(second["total_copies_available"], 9);
            assert_eq!(second["total_copies_reserved"], 1);

            // sp-23v6q1: color: is a: over the four colors, and query_help says so.
            let by_a = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": "a:command", "owned_only": true}),
                )
                .await
                .unwrap();
            let by_color = site
                .tool(
                    &t,
                    "search_cards",
                    json!({"q": "color:command", "owned_only": true}),
                )
                .await
                .unwrap();
            assert_eq!(by_a["results"], by_color["results"]);
            assert!(by_a["total"].as_i64().unwrap() > 0);
            let help = site
                .tool(&t, "query_help", json!({}))
                .await
                .unwrap()
                .to_string();
            for want in [
                "Common=0 < Uncommon=1 < Rare=2",
                "Legendary=3 < Special=4",
                "Vigilance, Command, Aggression and Cunning",
                "Heroism and Villainy are alignments, not colors",
                "t:unit excludes leaders",
                "(t:unit or t:leader)",
            ] {
                assert!(help.contains(want), "query_help lacks {want:?}");
            }
        }

        /// sp-orq075: leaders counted per color x alignment; a two-aspect Neutral leader counts
        /// under each color, a leader with no color under `none`, Neutral apart from Heroism.
        #[tokio::test]
        async fn collection_summary_leaders_by_color_and_alignment() {
            let mut site = Site::with_catalog(
                &["alice"],
                "INSERT INTO cards (card_id, name, card_type, aspects) \
                   VALUES ('l-hero', 'Hero', 'Leader', '[\"Vigilance\",\"Heroism\"]'), \
                          ('l-neutral', 'Neutral', 'Leader', '[\"Command\",\"Vigilance\"]'), \
                          ('l-bare', 'Bare', 'Leader', '[]'), \
                          ('l-villain', 'Villain', 'Leader', '[\"Villainy\"]'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('p-hero', 'l-hero', 'SOR', '9001', 'Standard', 'Normal'), \
                          ('p-neutral', 'l-neutral', 'SOR', '9002', 'Standard', 'Normal'), \
                          ('p-bare', 'l-bare', 'SOR', '9003', 'Standard', 'Normal'), \
                          ('p-villain', 'l-villain', 'SOR', '9004', 'Standard', 'Normal');",
            )
            .await;
            site.sign_in(member("alice@example.com")).await;
            for p in ["p-hero", "p-neutral", "p-bare", "p-villain"] {
                let body = json!({"printing_id": p, "source": "manual_id"}).to_string();
                let r = site.send("POST", "/api/collection", Some(&body)).await;
                assert_eq!(r.status(), StatusCode::CREATED);
            }
            let t = site.token_for("alice@example.com");
            let b = site
                .tool(&t, "collection_summary", json!({}))
                .await
                .unwrap()["breakdown"]["leaders"]["by_aspect_alignment"]
                .clone();
            let oa = json!({"distinct": 1, "owned": 1, "available": 1});
            assert_eq!(
                b,
                json!({
                    "Vigilance": {"Heroism": oa, "Neutral": oa},
                    "Command": {"Neutral": oa},
                    "none": {"Neutral": oa, "Villainy": oa},
                })
            );
        }

        /// sp-akeb45: leaders report distinct cards beside copies: three copies of one leader
        /// and one of another are distinct 2, owned 4.
        #[tokio::test]
        async fn collection_summary_leaders_report_distinct_cards() {
            let mut site = Site::with_catalog(
                &["alice"],
                "INSERT INTO cards (card_id, name, card_type, aspects) \
                   VALUES ('l-a', 'A', 'Leader', '[\"Vigilance\",\"Heroism\"]'), \
                          ('l-b', 'B', 'Leader', '[\"Vigilance\",\"Heroism\"]'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('p-a', 'l-a', 'SOR', '9001', 'Standard', 'Normal'), \
                          ('p-b', 'l-b', 'SOR', '9002', 'Standard', 'Normal');",
            )
            .await;
            site.sign_in(member("alice@example.com")).await;
            for p in ["p-a", "p-a", "p-a", "p-b"] {
                let body = json!({"printing_id": p, "source": "manual_id"}).to_string();
                let r = site.send("POST", "/api/collection", Some(&body)).await;
                assert_eq!(r.status(), StatusCode::CREATED);
            }
            let t = site.token_for("alice@example.com");
            let l = site
                .tool(&t, "collection_summary", json!({}))
                .await
                .unwrap()["breakdown"]["leaders"]
                .clone();
            let want = json!({"distinct": 2, "owned": 4, "available": 4});
            assert_eq!(l["by_alignment"]["Heroism"], want);
            assert_eq!(l["by_aspect"]["Vigilance"], want);
            assert_eq!(l["by_aspect_alignment"]["Vigilance"]["Heroism"], want);
        }

        /// sp-fn5iwc: one canonical form across the tools: ids as the catalog numbers them
        /// (`SOR-5`), the summary's set the card's own set (not a weekly-play `LAWP`), and the
        /// aspect names (Heroism, Villainy) for alignment.
        #[tokio::test]
        async fn mcp_ids_sets_and_alignment_use_one_vocabulary() {
            let mut site = Site::with_catalog(
                &["alice"],
                "INSERT INTO sets (set_code, name, release_date, sort_order) \
                   VALUES ('LAW', 'A Later Set', '2025-11-14', 9), \
                          ('LAWP', 'Later Weekly Play', '2025-11-20', 10); \
                 INSERT INTO cards (card_id, name, card_type, aspects) \
                   VALUES ('c-hero', 'Hero', 'Leader', '[\"Vigilance\",\"Heroism\"]'), \
                          ('c-vill', 'Villain', 'Leader', '[\"Aggression\",\"Villainy\"]'), \
                          ('c-none', 'Neutral', 'Unit', '[\"Vigilance\"]'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('h-law', 'c-hero', 'LAW', '131', 'Standard', 'Normal'), \
                          ('h-lawp', 'c-hero', 'LAWP', '21', 'Hyperspace', 'Foil'), \
                          ('v-sor', 'c-vill', 'SOR', '5', 'Standard', 'Normal'), \
                          ('n-sor', 'c-none', 'SOR', '12', 'Standard', 'Normal');",
            )
            .await;
            site.sign_in(member("alice@example.com")).await;
            let body = json!({"printing_id": "h-lawp", "source": "manual_id"}).to_string();
            let r = site.send("POST", "/api/collection", Some(&body)).await;
            assert_eq!(r.status(), StatusCode::CREATED);
            let t = site.token_for("alice@example.com");

            let b = site
                .tool(&t, "collection_summary", json!({}))
                .await
                .unwrap()["breakdown"]
                .clone();
            assert_eq!(
                b["by_set"],
                json!({"LAW": {"owned": 1, "available": 1}}),
                "the summary names the card's own set"
            );

            let hero = site
                .tool(&t, "get_card", json!({"id": "LAW-131"}))
                .await
                .unwrap();
            assert_eq!(hero["card"]["id"], "LAW-131");
            assert_eq!(hero["card"]["set"], "LAW");
            assert_eq!(hero["card"]["game"]["alignment"], "Heroism");
            let vill = site
                .tool(&t, "get_card", json!({"id": "SOR-005"}))
                .await
                .unwrap();
            assert_eq!(vill["card"]["id"], "SOR-5", "ids carry no zero padding");
            assert_eq!(vill["card"]["game"]["alignment"], "Villainy");
            let none = site
                .tool(&t, "get_card", json!({"id": "SOR-12"}))
                .await
                .unwrap();
            assert_eq!(none["card"]["game"]["alignment"], "Neutral");

            let tools = site.rpc(&t, "tools/list", json!({})).await;
            assert!(
                !tools.to_string().contains("SOR-005"),
                "schema examples use the returned id form"
            );
        }

        /// sp-jsizxw: search_cards is one row per card with its original printing's id, and
        /// `v:standard` leaves the Prestige and Serialized variants out.
        #[tokio::test]
        async fn mcp_search_is_one_row_per_card_with_the_original_id() {
            let site = Site::with_catalog(
                &["alice"],
                "INSERT INTO sets (set_code, name, release_date, sort_order) \
                   VALUES ('LAW', 'A Later Set', '2025-11-14', 9); \
                 INSERT INTO cards (card_id, name, card_type) VALUES ('c-repair', 'Repair', 'Event'); \
                 INSERT INTO cards (card_id, name, card_type) VALUES ('c-priest', 'Priestesses', 'Unit'); \
                 INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
                   VALUES ('r-law', 'c-repair', 'LAW', '75', 'Standard', 'Normal'), \
                          ('r-sor-h', 'c-repair', 'SOR', '300', 'Hyperspace', 'Normal'), \
                          ('r-sor-f', 'c-repair', 'SOR', '74', 'Standard', 'Foil'), \
                          ('r-sor', 'c-repair', 'SOR', '74', 'Standard', 'Normal'), \
                          ('p-std', 'c-priest', 'SOR', '90', 'Standard', 'Normal'), \
                          ('p-pre', 'c-priest', 'SOR', '500', 'Standard Prestige', 'Normal'), \
                          ('p-fpre', 'c-priest', 'SOR', '500', 'Foil Prestige', 'Foil'), \
                          ('p-ser', 'c-priest', 'SOR', '500', 'Serialized Prestige', 'Normal');",
            )
            .await;
            let token = site.token_for("alice@example.com");
            let all = site
                .tool(&token, "search_cards", json!({"q": "t:event OR t:unit"}))
                .await
                .unwrap();
            let rows = all["results"].as_array().unwrap();
            assert_eq!(all["total"], json!(3), "{all}");
            assert_eq!(rows.len(), 3);
            assert!(rows.iter().all(|r| r.get("printing_id").is_none()));
            let repair = rows.iter().find(|r| r["name"] == "Repair").unwrap();
            assert_eq!(repair["id"], "SOR-74", "the original, not the LAW reprint");

            let expanded = site
                .tool(
                    &token,
                    "search_cards",
                    json!({"q": "t:event OR t:unit", "expand_printings": true}),
                )
                .await
                .unwrap();
            assert_eq!(expanded["total"], json!(9), "{expanded}");
            assert!(
                expanded["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["id"] == "SOR-74" || r["id"] == "SOR-90" || r["id"] == "SOR-1")
            );

            let standard = site
                .tool(
                    &token,
                    "search_cards",
                    json!({"q": "v:standard", "expand_printings": true}),
                )
                .await
                .unwrap();
            let variants: Vec<_> = standard["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["variant"].as_str().unwrap().to_string())
                .collect();
            assert!(variants.iter().all(|v| v == "Standard"), "{variants:?}");
            let prestige = site
                .tool(
                    &token,
                    "search_cards",
                    json!({"q": "v:prestige", "expand_printings": true}),
                )
                .await
                .unwrap();
            assert_eq!(prestige["total"], json!(3), "{prestige}");
            // A prestige printing keeps its card's standard id, set and number (sp-wxj6lr).
            for r in prestige["results"].as_array().unwrap() {
                assert_eq!(r["id"], "SOR-90", "{r}");
                assert_eq!(r["set"], "SOR", "{r}");
                assert_eq!(r["number"], "90", "{r}");
                assert_eq!(r["printing_number"], "500", "{r}");
            }
            let hyper = site
                .tool(&token, "search_cards", json!({"q": "v:hyperspace"}))
                .await
                .unwrap();
            assert_eq!(hyper["total"], json!(1), "{hyper}");
            assert_eq!(hyper["results"][0]["id"], "SOR-74");
        }

        /// An unknown term gets the nearest valid ones, not an empty result.
        #[tokio::test]
        async fn a_misspelled_term_names_the_nearest_valid_ones() {
            let site = Site::new(&["alice"]).await;
            let t = site.token_for("alice@example.com");
            let e = site
                .tool(&t, "search_cards", json!({"q": "aspec:vigilance"}))
                .await
                .unwrap_err();
            assert!(e.contains("did you mean") && e.contains("'aspect:'"), "{e}");
        }

        /// Acceptance: the kit's conformance suite passes against a running dumpstar.
        #[tokio::test]
        async fn the_kit_conformance_suite_passes_against_a_running_site() {
            let site = Site::with_catalog(&["alice"], SECOND_CARD).await;
            let token = site.token_for("alice@example.com");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let router = site.router.clone();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            // The served help has the keyword list filled in, so the check reads the same text.
            let keywords = std::env::temp_dir().join(format!(
                "dumpstar-conformance-keywords-{}.json",
                std::process::id()
            ));
            std::fs::write(&keywords, dumpstar_db::search_meta::keywords_json()).unwrap();
            let report =
                dumpster_kit_mcp::conformance::run(&dumpster_kit_mcp::conformance::Config {
                    url: format!("http://{addr}/mcp"),
                    authorization: Some(format!("Bearer {token}")),
                    keywords_path: keywords,
                    paging_query: "t:unit".into(),
                })
                .await;
            assert!(report.passed(), "{report}");
        }
    }
}
