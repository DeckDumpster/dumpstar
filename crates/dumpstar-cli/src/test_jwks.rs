//! The test Cloudflare Access issuer: an RS256 JWKS server and token minting.
//!
//! Built only with the `test-jwks` feature, as the `dumpstar-test-jwks`
//! binary (`tests/lib/test-jwks.sh` builds and runs it). The harnesses that
//! start the shipped image present its tokens; the image never carries it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::{Parser, Subcommand};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KID: &str = "test-kid-1";
const AUD: &str = "test-aud-dumpstar";
const ISSUER: &str = "https://test.cloudflareaccess.com";

#[derive(Parser)]
#[command(name = "dumpstar-test-jwks", about = "Test Cloudflare Access issuer")]
pub struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a fresh key and <dir>/jwks.json, then serve the JWKS until killed
    /// (on PDH_JWKS_PORT, or a random port).
    Serve { dir: PathBuf },
    /// Print an RS256 token for <email>, signed by the key in <dir>.
    #[command(alias = "mint")]
    Issue {
        dir: PathBuf,
        email: String,
        /// Sign with a throwaway key the JWKS does not publish: the server must reject it.
        #[arg(long)]
        forged: bool,
    },
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Cmd::Serve { dir } => tokio::runtime::Runtime::new()?.block_on(serve(&dir)),
        Cmd::Issue { dir, email, forged } => {
            println!("{}", issue(&dir, &email, forged)?);
            Ok(())
        }
    }
}

fn b64(n: &rsa::BigUint) -> String {
    URL_SAFE_NO_PAD.encode(n.to_bytes_be())
}

async fn serve(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?;
    std::fs::write(
        dir.join("key.pem"),
        key.to_pkcs8_pem(LineEnding::LF)?.as_bytes(),
    )?;
    let body = json!({"keys": [{"kty": "RSA", "kid": KID, "n": b64(key.n()), "e": b64(key.e())}]})
        .to_string();

    let port: u16 = std::env::var("PDH_JWKS_PORT")
        .ok()
        .map(|p| p.parse())
        .transpose()
        .context("PDH_JWKS_PORT")?
        .unwrap_or(0);
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let port = listener.local_addr()?.port();

    let meta = json!({"port": port, "aud": AUD, "issuer": ISSUER, "kid": KID}).to_string();
    // jwks.json is written last: callers poll for it as the readiness signal.
    std::fs::write(dir.join("jwks.json"), &meta)?;
    println!("{meta}");

    loop {
        let (mut sock, _) = listener.accept().await?;
        let body = body.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            let resp = if head.starts_with("GET /cdn-cgi/access/certs ") {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
            };
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    }
}

fn issue(dir: &Path, email: &str, forged: bool) -> Result<String> {
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("jwks.json"))?)?;
    let pem = if forged {
        RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?
            .to_pkcs8_pem(LineEnding::LF)?
            .to_string()
    } else {
        std::fs::read_to_string(dir.join("key.pem"))?
    };
    let field = |k: &str| -> Result<String> {
        match meta[k].as_str() {
            Some(s) => Ok(s.to_string()),
            None => bail!("jwks.json has no {k}"),
        }
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = json!({
        "sub": format!("test-sub-{email}"),
        "email": email,
        "iss": field("issuer")?,
        "aud": [field("aud")?],
        "iat": now,
        "nbf": now,
        "exp": now + 3600,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(field("kid")?);
    Ok(encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes())?,
    )?)
}
