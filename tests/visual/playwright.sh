#!/usr/bin/env bash
#
# Run the visual suite against an instance that is already up.
#
# Installs the node deps on first use, then runs Playwright inside the
# Playwright container. `run.sh` calls this after standing up a throwaway
# instance. The instance must admit visual-test@example.com.
#
# Usage:
#   DUMPSTAR_BASE_URL=http://localhost:8099 bash tests/visual/playwright.sh [args...]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

: "${DUMPSTAR_BASE_URL:?DUMPSTAR_BASE_URL must point at a running instance}"

if [ ! -d node_modules ]; then
    echo "==> Installing visual-suite dependencies..."
    if [ -f package-lock.json ]; then npm ci; else npm install; fi
fi

# Chromium and its system libraries come from the Playwright image matching the
# @playwright/test that package-lock.json pins; the host needs neither
# (law-run-the-suite-in-a-container).
version="$(jq -er '.packages["node_modules/@playwright/test"].version' package-lock.json)"
image="mcr.microsoft.com/playwright:v${version}-noble"

# The whole tree, read-only, because the specs read outside this directory
# (routes.spec.ts walks frontend/src/routes); only this directory takes the
# report and the test results.
repo="$(cd "$SCRIPT_DIR/../.." && pwd)"

# The issuer is a host-built binary, so the token is minted out here. It lasts
# an hour, far longer than the suite.
if [ -z "${UI_TEST_ACCESS_TOKEN:-}" ] && [ -f "${UI_TEST_JWKS_DIR:-}/jwks.json" ]; then
    UI_TEST_ACCESS_TOKEN="$(bash "$repo/tests/lib/test-jwks.sh" issue "$UI_TEST_JWKS_DIR" visual-test@example.com)"
    export UI_TEST_ACCESS_TOKEN
fi

echo "==> Running the browser specs against ${DUMPSTAR_BASE_URL} in ${image}..."
# --network host: the instance publishes on the host's loopback.
exec podman run --rm --init --ipc=host --network host --security-opt label=disable \
    -v "$repo:$repo:ro" -v "$SCRIPT_DIR:$SCRIPT_DIR" -w "$SCRIPT_DIR" \
    -e DUMPSTAR_BASE_URL -e UI_TEST_ACCESS_TOKEN -e CI \
    "$image" npx --no-install playwright test "$@"
