//! The test Cloudflare Access issuer, built on dumpster-kit's `TestAccessFixture`.
//!
//! Built only with the `test-jwks` feature, as the `dumpstar-test-jwks`
//! binary (`tests/lib/test-jwks.sh` builds and runs it). The harnesses that
//! start the shipped image present its tokens; the image never carries it.
//!
//! The kit's fixture holds the key and signs; it binds loopback only, so `serve`
//! relays its JWKS onto the public port the containers reach and answers
//! `issue` over a loopback-only mint port recorded in `jwks.json`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use dumpster_kit_tenancy::testing::TestAccessFixture;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser)]
#[command(name = "dumpstar-test-jwks", about = "Test Cloudflare Access issuer")]
pub struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write <dir>/jwks.json, then serve the JWKS until killed
    /// (on PDH_JWKS_PORT, or a random port).
    Serve { dir: PathBuf },
    /// Print a token for <email> from the issuer serving <dir>.
    #[command(alias = "mint")]
    Issue {
        dir: PathBuf,
        email: String,
        /// Sign with a key the JWKS does not publish: the server must reject it.
        #[arg(long)]
        forged: bool,
    },
}

pub fn run(args: Args) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    match args.command {
        Cmd::Serve { dir } => rt.block_on(serve(dir)),
        Cmd::Issue { dir, email, forged } => {
            println!("{}", rt.block_on(issue(&dir, &email, forged))?);
            Ok(())
        }
    }
}

async fn serve(dir: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&dir)?;
    let fx = TestAccessFixture::new().await;
    let upstream = fx
        .jwks_url
        .strip_prefix("http://")
        .and_then(|u| u.split('/').next())
        .context("fixture jwks_url")?
        .to_string();

    let port: u16 = std::env::var("PDH_JWKS_PORT")
        .ok()
        .map(|p| p.parse())
        .transpose()
        .context("PDH_JWKS_PORT")?
        .unwrap_or(0);
    let public = TcpListener::bind(("0.0.0.0", port)).await?;
    let mint = TcpListener::bind(("127.0.0.1", 0)).await?;

    let meta = json!({
        "port": public.local_addr()?.port(),
        "mint_port": mint.local_addr()?.port(),
        "aud": fx.aud,
        "issuer": fx.team_domain,
    })
    .to_string();
    // jwks.json is written last: callers poll for it as the readiness signal.
    std::fs::write(dir.join("jwks.json"), &meta)?;
    println!("{meta}");

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = public.accept().await else {
                continue;
            };
            let upstream = upstream.clone();
            tokio::spawn(async move {
                if let Ok(mut up) = TcpStream::connect(upstream).await {
                    let _ = copy_bidirectional(&mut sock, &mut up).await;
                }
            });
        }
    });

    loop {
        let (mut sock, _) = mint.accept().await?;
        let mut buf = [0u8; 4096];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        let line = head.lines().next().unwrap_or_default();
        // "GET /<forged|valid>/<email> HTTP/1.1"
        let path = line.split_whitespace().nth(1).unwrap_or_default();
        let token = match path.trim_start_matches('/').split_once('/') {
            Some(("valid", email)) => Some(fx.valid_token(email)),
            Some(("forged", email)) => Some(fx.unknown_kid_token(email)),
            _ => None,
        };
        let resp = match token {
            Some(t) => format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{t}",
                t.len()
            ),
            None => {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
            }
        };
        let _ = sock.write_all(resp.as_bytes()).await;
    }
}

async fn issue(dir: &std::path::Path, email: &str, forged: bool) -> Result<String> {
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("jwks.json"))
            .context("no jwks.json: is serve running?")?,
    )?;
    let Some(port) = meta["mint_port"].as_u64() else {
        bail!("jwks.json has no mint_port");
    };
    let kind = if forged { "forged" } else { "valid" };
    let mut sock = TcpStream::connect(("127.0.0.1", port as u16)).await?;
    sock.write_all(format!("GET /{kind}/{email} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .await?;
    let mut out = String::new();
    sock.read_to_string(&mut out).await?;
    match out.split_once("\r\n\r\n") {
        Some((head, body)) if head.starts_with("HTTP/1.1 200") => Ok(body.to_string()),
        _ => bail!("mint refused: {out}"),
    }
}
