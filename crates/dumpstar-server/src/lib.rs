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

mod routes;

use dumpster_kit_tenancy::{AccessState, HttpError, Tenants};
pub use dumpster_kit_tenancy::{access, tenant};

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
    /// Cloudflare Access JWT validation state (JWKS cache + config).
    pub(crate) access: Arc<AccessState>,
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
    let id = tenant::current()?;
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
    let authenticated_api = routes::authenticated_api_router()
        .route_layer(middleware::from_fn_with_state(
            state.tenants.clone(),
            tenant::layer,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.access.clone(),
            access::layer,
        ));
    let public_api = routes::public_api_router();
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
        .nest_service("/sym", ServeDir::new(data_dir.join("symbols")))
        .route_service("/robots.txt", ServeFile::new(static_dir.join("robots.txt")))
        .with_state(state)
        .fallback(get(spa))
        .layer(middleware::from_fn(api_responses_are_private))
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
    /// Cloudflare Access configuration. Mandatory: there is no mode without
    /// it. A JWKS fetch failure is a startup failure. See
    /// `access::AccessConfig::from_env`.
    pub access: access::AccessConfig,
}

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
    // Cloudflare Access JWT validation: primes the JWKS cache; a failed fetch
    // is a startup failure.
    let access = AccessState::new(cfg.access).await?;
    let tenants = Tenants::new(
        dumpstar_db::app::app().clone(),
        cfg.tenants_dir,
        cfg.shared_db,
        &cfg.registry_db,
        dumpstar_db::user_spec(),
    )?;
    let state = AppState {
        tenants: Arc::new(tenants),
        registry,
        flags,
        data_dir: Arc::new(cfg.data_dir.clone()),
        access,
    };
    let addr = SocketAddr::new(cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("dumpstar serving on http://{addr}");
    axum::serve(listener, app(state, cfg.static_dir, cfg.data_dir)).await?;
    Ok(())
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
            access,
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
                access: fx.access.clone(),
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
    }
}
