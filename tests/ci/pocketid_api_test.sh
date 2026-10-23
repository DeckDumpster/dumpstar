#!/usr/bin/env bash
# tier: T0
# covers: infra/*
# tests/ci/pocketid_api_test.sh — infra/edge/pocketid-api.sh registers an MCP audience as a Pocket ID API
# with CIMD allowed, adopts one that exists, and removes only by audience. A stub curl records calls.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail=0
bad() { echo "FAIL: $*" >&2; fail=1; }

mkdir "$tmp/bin"
cat >"$tmp/bin/curl" <<'STUB'
#!/usr/bin/env bash
method=GET; url=
while [ $# -gt 0 ]; do
  case "$1" in -X) method="$2"; shift ;; --data) echo "BODY $2" >>"$STUB_LOG"; shift ;; http*) url="$1" ;; esac
  shift
done
echo "$method $url" >>"$STUB_LOG"
case "$method $url" in
"GET "*/api/apis\?*) cat "$STUB_LIST" ;;
"POST "*/api/apis) echo '{"id":"new1"}' ;;
*) echo '{}' ;;
esac
STUB
chmod +x "$tmp/bin/curl"
export PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_LIST="$tmp/list" POCKETID_API_TOKEN=t
run() { : >"$STUB_LOG"; bash "$root/infra/edge/pocketid-api.sh" "$@" >/dev/null; }

echo '{"data":[]}' >"$STUB_LIST"
run ensure https://id.example env-1.example/mcp "dumpstar MCP env-1"
grep -q '^POST https://id.example/api/apis$' "$STUB_LOG" || bad "absent audience was not created"
grep -q '^PUT .*/api/apis/new1/cimd-access$' "$STUB_LOG" || bad "CIMD access not set on the new API"
grep -q 'BODY {"enabled":true' "$STUB_LOG" || bad "CIMD access not enabled"

echo '{"data":[{"id":"prod1","resource":"https://star.example/mcp"}]}' >"$STUB_LIST"
run ensure https://id.example https://star.example/mcp "dumpstar MCP"
grep -q '^POST' "$STUB_LOG" && bad "existing audience was duplicated"
grep -q '^PUT .*/api/apis/prod1/cimd-access$' "$STUB_LOG" || bad "existing API not set to allow CIMD"

run remove https://id.example https://star.example/mcp x
grep -q '^DELETE .*/api/apis/prod1$' "$STUB_LOG" || bad "remove did not delete the API"
echo '{"data":[]}' >"$STUB_LIST"
run remove https://id.example https://gone.example/mcp x
grep -q '^DELETE' "$STUB_LOG" && bad "remove deleted with nothing registered"

exit "$fail"
