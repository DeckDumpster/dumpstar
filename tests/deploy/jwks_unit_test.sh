#!/usr/bin/env bash
# tier: T2
# covers: tests/lib/access.sh deploy/setup.sh
# tests/deploy/jwks_unit_test.sh — the test JWKS transient unit starts reliably
# (sp-2p04td): a failed unit left under the retired fixed name does not block it,
# two setups of one instance prefix running at once each get a jwks.json, and a
# unit that cannot start explains itself. Needs a systemd user manager.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
# shellcheck source=tests/lib/access.sh
. "$REPO_DIR/tests/lib/access.sh"

if ! systemctl --user show-environment >/dev/null 2>&1; then
	echo "SKIP: no systemd user manager"
	exit 0
fi

PREFIX="dumpstar-test-jwks-unittest-$$"
ROOT="$(mktemp -d)"
cleanup() {
	for d in a b c; do access_unit_stop "$ROOT/$d/jwks" 2>/dev/null || true; done
	systemctl --user stop "$PREFIX" 2>/dev/null || true
	systemctl --user reset-failed "$PREFIX" 2>/dev/null || true
	rm -rf "$ROOT"
}
trap cleanup EXIT

BIN="$(bash "$REPO_DIR/tests/lib/test-jwks.sh" build)"
fail() { echo "FAIL: $*" >&2; exit 1; }

echo "==> a failed unit under the old fixed name does not block the start"
systemd-run --user --quiet --unit "$PREFIX" false || true
for _ in $(seq 1 50); do
	systemctl --user is-failed --quiet "$PREFIX" && break
	sleep 0.1
done
systemctl --user is-failed --quiet "$PREFIX" || fail "positive control: the fixed-name unit should be failed"
mkdir -p "$ROOT/a/jwks"
access_unit_start "$ROOT/a/jwks" "$PREFIX" "$BIN" || fail "start refused after a failed fixed-name unit"
[[ -f "$ROOT/a/jwks/jwks.json" ]] || fail "no jwks.json"
[[ "$ACCESS_UNIT" != "$PREFIX" ]] || fail "unit name is not unique"

echo "==> two setups at once both get a jwks.json"
mkdir -p "$ROOT/b/jwks" "$ROOT/c/jwks"
(access_unit_start "$ROOT/b/jwks" "$PREFIX" "$BIN") &
p1=$!
(access_unit_start "$ROOT/c/jwks" "$PREFIX" "$BIN") &
p2=$!
wait "$p1" || fail "first concurrent start failed"
wait "$p2" || fail "second concurrent start failed"
[[ -f "$ROOT/b/jwks/jwks.json" && -f "$ROOT/c/jwks/jwks.json" ]] || fail "a concurrent start wrote no jwks.json"

echo "==> a unit that exits at once explains itself"
mkdir -p "$ROOT/d/jwks"
if err="$(access_unit_start "$ROOT/d/jwks" "$PREFIX" /nonexistent/jwks-bin 2>&1)"; then
	fail "start of a broken binary reported success"
fi
grep -q "dumpstar-test-jwks" <<<"$err" || fail "failure did not name the unit: $err"
access_unit_stop "$ROOT/d/jwks"
echo "PASS"
