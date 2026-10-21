#!/usr/bin/env bash
# tier: T1
# covers: crates/dumpstar-hold-proxy deploy/front.sh deploy/deploy.sh
# tests/deploy/hold_proxy_test.sh — a deploy must not fail requests (sp-slm2ib).
# Hermetic: a stand-in backend that is stopped and started like the container, and a steady
# request loop (10 requests/second) across the restart.
#   §1 WITHOUT the proxy, the loop sees refused requests (today's deploy: Cloudflare 502)
#   §2 WITH the proxy in front, the same restart records zero non-2xx responses
#   §3 the proxy logs the window it held requests for (the deploy log reads it)
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
scratch="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null; [ -f "$scratch/backend.pid" ] && kill "$(cat "$scratch/backend.pid")" 2>/dev/null; rm -rf "$scratch"; true' EXIT
FRONT=18491 BACK=18492
fail=0
check() { if [ "$2" = "$3" ]; then echo "  PASS  $1 (= $3)"; else echo "  FAIL  $1 (expected $2, got $3)"; fail=1; fi; }

cargo build -q -p dumpstar-hold-proxy --release --bins --examples --manifest-path "$REPO/Cargo.toml"
TARGET="$(cargo metadata --format-version 1 --no-deps --manifest-path "$REPO/Cargo.toml" | sed -n 's|.*"target_directory":"\([^"]*\)".*|\1|p')/release"
BACKEND="$TARGET/examples/stub_backend"
PROXY="$TARGET/dumpstar-hold-proxy"

# loop <port> <seconds>: 10 requests/second; prints the count of non-2xx responses.
loop() {
    local port="$1" n="$2" bad=0 i code
    for ((i = 0; i < n; i++)); do
        code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 "http://127.0.0.1:${port}/" || true)"
        [ "$code" = 200 ] || bad=$((bad + 1))
        sleep 0.1
    done
    echo "$bad"
}
# restart_backend: stop it, stay down for 2s (the container restart), start it again.
restart_backend() {
    sleep 1
    kill "$(cat "$scratch/backend.pid")"; wait "$(cat "$scratch/backend.pid")" 2>/dev/null || true
    sleep 2
    "$BACKEND" "$BACK" & echo $! >"$scratch/backend.pid"
}
wait_up() { for _ in $(seq 100); do curl -s -o /dev/null "http://127.0.0.1:$1/" && return 0; sleep 0.1; done; return 1; }

"$BACKEND" "$BACK" & echo $! >"$scratch/backend.pid"
wait_up "$BACK"

echo "=== §1 no proxy: the restart refuses requests ==="
restart_backend & rp=$!
bad="$(loop "$BACK" 60)"; wait $rp
check "requests refused during a bare restart (seen to fail first)" "yes" "$([ "$bad" -gt 0 ] && echo yes || echo no)"
wait_up "$BACK"

echo "=== §2 proxy in front: zero non-2xx ==="
"$PROXY" --listen "127.0.0.1:$FRONT" --upstream "127.0.0.1:$BACK" --hold 30 >"$scratch/proxy.log" &
wait_up "$FRONT"
restart_backend & rp=$!
bad="$(loop "$FRONT" 80)"; wait $rp
check "non-2xx responses through the proxy across the restart" "0" "$bad"

echo "=== §3 the window is logged ==="
check "proxy logged a held connection" "yes" "$(grep -q '^held [0-9]' "$scratch/proxy.log" && echo yes || echo no)"
exit $fail
