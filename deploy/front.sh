#!/usr/bin/env bash
# The hold proxy for an instance (sp-slm2ib): listens on the instance's origin port, pipes to the
# container's backend port. Run by dumpstar-front@<instance>.service. Prod only: the tunnel
# targets the origin port, and the container publishes the backend port instead.
# The proxy is the dumpstar-hold-proxy crate (sp-io9c7k); built here when this checkout has no
# current binary.
set -euo pipefail
DIR="$(dirname "${BASH_SOURCE[0]}")"
# shellcheck source=deploy/units-lib.sh
. "$DIR/units-lib.sh"
[ "${1:-}" = prod ] || { echo "front.sh: only prod has a hold proxy" >&2; exit 2; }
cargo build --release --locked -q -p dumpstar-hold-proxy --manifest-path "$DIR/../Cargo.toml"
exec "${CARGO_TARGET_DIR:-$DIR/../target}/release/dumpstar-hold-proxy" \
    --listen "127.0.0.1:${DUMPSTAR_PROD_PORT}" \
    --upstream "127.0.0.1:${DUMPSTAR_PROD_BACKEND_PORT}" \
    --hold "${DUMPSTAR_FRONT_HOLD_SECONDS:-90}"
