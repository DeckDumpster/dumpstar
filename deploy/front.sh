#!/usr/bin/env bash
# The hold proxy for an instance (sp-slm2ib): listens on the instance's origin port, pipes to the
# container's backend port. Run by dumpstar-front@<instance>.service. Prod only: the tunnel
# targets the origin port, and the container publishes the backend port instead.
set -euo pipefail
# shellcheck source=deploy/units-lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/units-lib.sh"
[ "${1:-}" = prod ] || { echo "front.sh: only prod has a hold proxy" >&2; exit 2; }
exec python3 "$(dirname "${BASH_SOURCE[0]}")/hold-proxy.py" \
    --listen "127.0.0.1:${DUMPSTAR_PROD_PORT}" \
    --upstream "127.0.0.1:${DUMPSTAR_PROD_BACKEND_PORT}" \
    --hold "${DUMPSTAR_FRONT_HOLD_SECONDS:-90}"
