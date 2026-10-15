#!/usr/bin/env bash
# The ui tier: every tests/ui replay implementation, deterministically.
#
# Builds `dumpstar` and the SvelteKit frontend from THIS tree, seeds the SWU
# fixture, starts the server against a throwaway Access JWKS, and runs the
# intents harness in replay-only mode (UI_REPLAY_ONLY=1): Playwright + chromium,
# no Vision, no ANTHROPIC_API_KEY, no podman. Intents with no implementation are
# named in vision-only.txt and skipped; that list may only shrink, and an intent
# that is neither implemented nor listed fails the run. A server that does not
# answer is a red, not a skip.
#
#   bash tests/ui/run.sh [playwright args...]     e.g. -g browse_grid_fits_viewport
#
# Needs: cargo, node 22.18+, python3 with cryptography and PyJWT, curl, and
# chromium's system libraries (`npx playwright install --with-deps` fetches them
# where apt is available; set UI_SKIP_BROWSER_INSTALL=1 where they are present).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
exec > >(tee -a "$root/.ui-run-debug.log") 2>&1
pids=()
cleanup() {
    for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    rm -rf "$work"
}
trap cleanup EXIT

step() { printf '\n==> %s\n' "$*"; }

step "ui: build dumpstar"
(cd "$root" && cargo build --quiet -p dumpstar-cli)
bin="${CARGO_TARGET_DIR:-$root/target}/debug/dumpstar"
[ -x "$bin" ] || { echo "ERROR: $bin was not built" >&2; exit 1; }

step "ui: build the frontend"
(cd "$root/frontend" && npm ci --silent && npm run build --silent)

step "ui: seed the SWU fixture"
home="$work/home"
mkdir -p "$home/tenants"
bash "$root/tests/ui/fixtures/build.sh" "$work/fixture"
cp "$work/fixture/shared.sqlite" "$home/shared.sqlite"
cp "$work/fixture/collection.sqlite" "$home/tenants/collection.sqlite"

step "ui: start the test Access JWKS and the server"
jwks="$work/jwks"
mkdir -p "$jwks"
python3 "$root/tests/lib/test_jwks.py" serve "$jwks" >/dev/null &
pids+=("$!")
for _ in $(seq 1 100); do [ -f "$jwks/jwks.json" ] && break; sleep 0.1; done
[ -f "$jwks/jwks.json" ] || { echo "ERROR: the test JWKS server wrote no jwks.json (python3 needs cryptography and PyJWT)" >&2; exit 1; }
meta() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$jwks/jwks.json" "$1"; }
port="${UI_TEST_PORT:-18080}"

export DUMPSTAR_HOME="$home"
export DUMPSTAR_USER=collection
export DUMPSTAR_ACCESS_TEAM_DOMAIN="$(meta issuer)"
export DUMPSTAR_ACCESS_AUD="$(meta aud)"
export DUMPSTAR_ACCESS_JWKS_URL="http://127.0.0.1:$(meta port)/cdn-cgi/access/certs"
export DUMPSTAR_STATIC_DIR="$root/frontend/build"
"$bin" serve --host 127.0.0.1 --port "$port" >"$work/server.log" 2>&1 &
pids+=("$!")
up=""
for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$port/" >/dev/null 2>&1; then up=yes; break; fi
    kill -0 "${pids[1]}" 2>/dev/null || break
    sleep 0.2
done
if [ -z "$up" ]; then
    echo "ERROR: the server did not come up; its log:" >&2
    cat "$work/server.log" >&2
    exit 1
fi

step "ui: smoke — the authenticated API answers"
token="$(python3 "$root/tests/lib/test_jwks.py" issue "$jwks" ui-test@example.com)"
for ep in sets collection/value-history?dimension=all; do
    curl -sS -m 20 -o /dev/null -w "    /api/$ep -> %{http_code} in %{time_total}s\n" \
        -H "Cf-Access-Jwt-Assertion: $token" "http://127.0.0.1:$port/api/$ep" || true
done

step "ui: run the replay implementations"
cd "$root/tests/ui"
npm ci --silent
if [ -z "${UI_SKIP_BROWSER_INSTALL:-}" ]; then
    npx playwright install --with-deps chromium
fi
export UI_REPLAY_ONLY=1
export UI_TEST_BASE_URL="http://127.0.0.1:$port"
export UI_TEST_DUMPSTAR_BIN="$bin"
export UI_TEST_ACCESS_TOKEN="$token"
rc=0
npx playwright test -g "browse_back_preserves_page_param" "$@" || rc=$?
if [ "$rc" -ne 0 ]; then
    echo "--- server log (tail) ---" >&2
    tail -n 40 "$work/server.log" >&2 || true
fi
exit "$rc"
