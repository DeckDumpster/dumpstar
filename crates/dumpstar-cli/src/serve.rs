//! `dumpstar serve` — start the HTTP server.
//!
//! `setup` uses `reqwest::blocking`, which cannot run inside a Tokio
//! runtime, so the CLI's `main` stays synchronous and `serve` spins up its
//! own runtime here.

use std::net::IpAddr;
use std::path::PathBuf;

use dumpstar_server::ServeConfig;

/// Arguments for `dumpstar serve`.
#[derive(clap::Args)]
pub struct ServeArgs {
    /// Address to bind. Defaults to localhost; the container deployment
    /// passes `0.0.0.0`.
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,

    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Directory holding the built SvelteKit SPA. Defaults to
    /// `$DUMPSTAR_STATIC_DIR`, else `frontend/build`.
    #[arg(long, value_name = "DIR")]
    static_dir: Option<PathBuf>,
}

/// Execute `dumpstar serve`. Database paths come from `DUMPSTAR_HOME`.
pub fn run(args: ServeArgs) -> anyhow::Result<()> {
    // Read first so a server without a sign-in refuses before touching any data. Any
    // `DUMPSTAR_OIDC_*` variable selects Pocket ID, and then every one of them is required;
    // with none, Cloudflare Access is what the server runs behind.
    let (oidc, access) = if std::env::vars().any(|(k, _)| k.starts_with("DUMPSTAR_OIDC_")) {
        (
            Some(dumpstar_server::OidcConfig::from_env("DUMPSTAR")?),
            None,
        )
    } else {
        let access = dumpstar_server::access::AccessConfig::from_env(dumpstar_db::app::app())
            .map_err(|e| {
                anyhow::anyhow!("{e}; or set DUMPSTAR_OIDC_* to sign in with Pocket ID")
            })?;
        (None, Some(access))
    };
    let cfg = ServeConfig {
        tenants_dir: dumpstar_db::tenants_dir()?,
        registry_db: dumpstar_db::registry_db_path()?,
        shared_db: dumpstar_db::shared_db_path()?,
        data_dir: dumpstar_db::dumpstar_home()?,
        static_dir: args.static_dir.unwrap_or_else(|| {
            std::env::var("DUMPSTAR_STATIC_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("frontend/build"))
        }),
        host: args.host,
        port: args.port,
        oidc,
        access,
        mcp_resource: std::env::var("DUMPSTAR_MCP_RESOURCE")
            .ok()
            .filter(|v| !v.trim().is_empty()),
    };
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(dumpstar_server::serve(cfg))
}
