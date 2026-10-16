//! `dumpstar-test-jwks` — the test Cloudflare Access issuer (feature `test-jwks`).

#[path = "test_jwks.rs"]
mod test_jwks;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    test_jwks::run(test_jwks::Args::parse())
}
