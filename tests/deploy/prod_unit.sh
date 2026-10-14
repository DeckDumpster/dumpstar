#!/usr/bin/env bash
# tests/deploy/prod_unit.sh — the production Quadlet unit, against real podman and
# real systemd (design row S5). Container tier.
#
# Builds a throwaway instance through the repository's own tooling
# (`deploy/setup.sh <name> --test`; any name but `prod`) and holds three things:
#
#   1. THE UNIT STARTS and serves /health on loopback.
#   2. LOOPBACK ONLY: the same port answers on 127.0.0.1 and does NOT answer on the
#      host's other address. A positive control proves the probe can see an exposed
#      listener (a probe that cannot fail proves nothing).
#   3. THE SERVING CHECK FAILS WHEN THE CONTAINER IS KILLED. deploy/heartbeat.sh
#      (the unit behind dumpstar-heartbeat@.timer) exits 0 while the origin serves
#      and non-zero the moment it does not.
#
# The server refuses to start without Cloudflare Access, so the instance is
# configured against tests/lib/test_jwks.py, a real RS256 JWKS server. No part of
# this touches prod or the public hostname.
#
#   bash tests/deploy/prod_unit.sh [instance]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

# shellcheck source=tests/lib/ports.sh
. "$REPO_DIR/tests/lib/ports.sh"
# shellcheck source=tests/lib/wait.sh
. "$REPO_DIR/tests/lib/wait.sh"
# shellcheck source=deploy/diagnostics-lib.sh
. "$REPO_DIR/deploy/diagnostics-lib.sh"

INSTANCE="${1:-produnit-$(printf '%s' "$REPO_DIR" | sha1sum | cut -c1-6)}"
if [ "$INSTANCE" = prod ]; then
	echo "refusing: this test never touches the prod instance" >&2
	exit 2
fi
SERVICE="dumpstar-${INSTANCE}"
CONTAINER="systemd-${SERVICE}"
CONF_DIR="${HOME}/.config/dumpstar/${INSTANCE}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/produnit.XXXXXX")"
JWKS_PID="" EXPOSED_PID=""

pass=0
fail=0
check() { # check <label> <expected> <actual>
	if [[ "$2" == "$3" ]]; then
		echo "  PASS  $1 (= $3)"
		pass=$((pass + 1))
	else
		echo "  FAIL  $1 (expected $2, got $3)"
		fail=$((fail + 1))
	fi
}
log() { printf '\n=== %s ===\n' "$*"; }

# shellcheck disable=SC2329  # invoked via trap
cleanup() {
	[ -n "$JWKS_PID" ] && kill "$JWKS_PID" 2>/dev/null || true
	[ -n "$EXPOSED_PID" ] && kill "$EXPOSED_PID" 2>/dev/null || true
	bash "$REPO_DIR/deploy/teardown.sh" "$INSTANCE" --purge >/dev/null 2>&1 || true
	rm -rf "$WORK"
}
trap cleanup EXIT

APP_PORT="$(free_port)"
JWKS_PORT="$(free_port)"

log "setup: a --test instance on loopback port ${APP_PORT}"
bash "$REPO_DIR/deploy/teardown.sh" "$INSTANCE" --purge >/dev/null 2>&1 || true
bash "$REPO_DIR/deploy/setup.sh" "$INSTANCE" "$APP_PORT" --test

PDH_JWKS_PORT="$JWKS_PORT" python3 -I "$REPO_DIR/tests/lib/test_jwks.py" serve "$WORK/jwks" >"$WORK/jwks.out" &
JWKS_PID=$!
wait_until 20 0.25 test -s "$WORK/jwks.out" || true
AUD="$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["aud"])' "$WORK/jwks/jwks.json")"

# Real values, no leading '-' to lean on and no inline '#': this is the file the
# unit's EnvironmentFile= reads.
cat >"$CONF_DIR/access.env" <<EOF
DUMPSTAR_ACCESS_TEAM_DOMAIN=https://test.cloudflareaccess.com
DUMPSTAR_ACCESS_AUD=${AUD}
DUMPSTAR_ACCESS_JWKS_URL=http://host.containers.internal:${JWKS_PORT}/cdn-cgi/access/certs
EOF
chmod 600 "$CONF_DIR/access.env"

log "1. the unit starts and serves /health on loopback"
systemctl --user start "$SERVICE" || { dump_unit_diagnostics "$SERVICE"; exit 1; }
health() { curl -sf -m 3 -o /dev/null "http://127.0.0.1:${APP_PORT}/health"; }
up=down
wait_until 60 0.25 health && up=up || true
check "the unit serves /health on 127.0.0.1:${APP_PORT}" up "$up"
if [ "$up" != up ]; then
	dump_unit_diagnostics "$SERVICE"
	printf '\n=== %d passed, %d failed ===\n' "$pass" "$((fail + 1))"
	exit 1
fi
check "the body is ok" ok "$(curl -sf -m 3 "http://127.0.0.1:${APP_PORT}/health")"
check "the unit publishes loopback only" 1 \
	"$(grep -c "^PublishPort=127\.0\.0\.1:${APP_PORT}:8080$" "${HOME}/.config/containers/systemd/${SERVICE}.container" || true)"

log "2. the port does not answer on the host's other address"
OTHER_IP="$(hostname -I 2>/dev/null | tr ' ' '\n' | grep -v '^127\.' | grep -E '^[0-9]+\.' | head -1 || true)"
if [ -z "$OTHER_IP" ]; then
	echo "  SKIP  no non-loopback IPv4 address on this host"
else
	# Positive control: a listener bound to 0.0.0.0 IS seen on that address.
	CONTROL_PORT="$(free_port)"
	python3 -I -m http.server "$CONTROL_PORT" --bind 0.0.0.0 --directory "$WORK" >/dev/null 2>&1 &
	EXPOSED_PID=$!
	seen=no
	wait_until 10 0.25 curl -s -m 2 -o /dev/null "http://${OTHER_IP}:${CONTROL_PORT}/" && seen=yes || true
	check "control: an exposed listener is reachable on ${OTHER_IP}" yes "$seen"
	exposed=no
	curl -s -m 3 -o /dev/null "http://${OTHER_IP}:${APP_PORT}/health" && exposed=yes || true
	check "the unit's port is NOT reachable on ${OTHER_IP}" no "$exposed"
fi

log "3. the serving check fails when the container is killed"
rc=0
bash "$REPO_DIR/deploy/heartbeat.sh" "$INSTANCE" >/dev/null 2>&1 || rc=$?
check "serving check passes while the origin serves" 0 "$rc"

podman kill "$CONTAINER" >/dev/null
rc=0
bash "$REPO_DIR/deploy/heartbeat.sh" "$INSTANCE" >/dev/null 2>&1 || rc=$?
check "serving check fails with the container killed" 1 "$rc"

# And it recovers: the unit restarts the container (Restart=on-failure), so the
# check going red was the outage, not a check that can only ever say no.
rc=1
retry() { bash "$REPO_DIR/deploy/heartbeat.sh" "$INSTANCE" >/dev/null 2>&1; }
wait_until 90 1 retry && rc=0 || true
check "serving check passes again once the unit restarts the container" 0 "$rc"

printf '\n=== %d passed, %d failed ===\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
