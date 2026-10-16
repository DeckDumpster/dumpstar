#!/usr/bin/env bash
# Build (once) and exec the Rust test Access issuer, `dumpstar-test-jwks`
# (crates/dumpstar-cli, feature test-jwks):
#   test-jwks.sh serve <dir>                  write <dir>/jwks.json, serve the JWKS
#   test-jwks.sh issue <dir> <email> [--forged]   print a token (--forged: a key the JWKS lacks)
# DUMPSTAR_TEST_JWKS_BIN names a prebuilt binary and skips the build. The binary
# is exec'd, so a kill of this script's PID stops the server.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ -z "${DUMPSTAR_TEST_JWKS_BIN:-}" ]]; then
	cargo build --quiet --manifest-path "$repo/Cargo.toml" -p dumpstar-cli \
		--features test-jwks --bin dumpstar-test-jwks >&2
	tdir="$(cargo metadata --manifest-path "$repo/Cargo.toml" --format-version 1 --no-deps | jq -r .target_directory)"
	DUMPSTAR_TEST_JWKS_BIN="$tdir/debug/dumpstar-test-jwks"
fi
exec "$DUMPSTAR_TEST_JWKS_BIN" "$@"
