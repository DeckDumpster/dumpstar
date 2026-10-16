#!/usr/bin/env bash
# Container-tier gate: what the SHIPPED image answers to multi-tenant requests,
# using Cloudflare Access JWT authentication (db-ae5u).
#
# Run by deploy/ci.sh. Standalone:
#   bash tests/tenants/handles.sh          # ~1min after the image is warm
#   KEEP=1 bash tests/tenants/handles.sh   # leave WORK + the container up
#
# ── WHY THIS EXISTS ─────────────────────────────────────────────────────────
# The auth layer (db-ae5u) changed multi-tenant resolution from a plain
# x-dumpstar-tenant header to Cloudflare Access JWT verification. This gate
# runs the shipped image against a TEST JWKS server (tests/lib/test_jwks.py)
# that issues real RS256 tokens, so it exercises the actual JWT verification
# path rather than a stub. The isolation claim rests on:
#
#   - the ONLY way to identify a tenant is the email from a verified JWT, and
#   - the ONLY way to produce a VerifiedIdentity (access::layer's output) is to
#     present a valid JWT to the access middleware — the constructor is private.
#
# This gate is the container-tier complement to the unit test
# `one_tenant_cannot_reach_another_tenants_collection` in
# crates/dumpstar-server/src/lib.rs.
#
# ── WHAT IT ASSERTS ─────────────────────────────────────────────────────────
#   §3 A JWKS server is started; alice's email is bound in the data directory.
#   §4 Without Access vars the server refuses to start — Cloudflare Access
#      is required, not optional, and there is no switch that changes that.
#   §5 A valid JWT for a bound email is served — the happy path.
#   §6 A valid JWT for an unbound email is a 403, naming the fix.
#   §7 An invalid JWT is a 401 — the server rejects it before resolving.
#   §8 No JWT at all is a 401 — there is no ambient identity.
#   §9 Nothing above provisioned anything new.
#  §10 Detach removes access and reattach restores it; purge refuses a live
#      row and a handle.
#
# Prod-safe: its own image tag, container name, temp directory and port. It
# touches no dumpstar-* unit, no dumpstar-*-data volume, no bucket.
set -euo pipefail

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
FIXTURES="${REPO_DIR}/tests/ui/fixtures"

# The shipped image, built here or — when deploy/ci.sh already built it once for
# every gate in the run — tagged from that one. See deploy/image-lib.sh.
# shellcheck source=deploy/image-lib.sh
. "${REPO_DIR}/deploy/image-lib.sh"
# Bounded condition polling, in one place for every harness (pd-86er).
# shellcheck source=tests/lib/wait.sh
. "${REPO_DIR}/tests/lib/wait.sh"

# PER-CHECKOUT, for the reason deploy/ci.sh derives its instance the same way:
# several polecats run this concurrently from their own worktrees, and a fixed
# container name means run B's opening `podman rm -f` kills run A mid-suite.
SUFFIX="${PDH_SUFFIX:-$(printf '%s' "$REPO_DIR" | sha1sum | cut -c1-8)}"
IMAGE="localhost/dumpstar:handles-${SUFFIX}"
APP_CTR="dumpstar-handles-${SUFFIX}"
# The host port is PODMAN'S to pick (`-p 127.0.0.1::8080`), read back off the
# container by `start_app`.
PORT=""

WORK=${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/pd-handles.XXXXXX")}
DATA="$WORK/data"
JWKS_DIR="$WORK/jwks"
JWKS_PID=""

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
	if [[ -n "${KEEP:-}" ]]; then
		echo
		echo "KEEP=1 — leaving $APP_CTR, $IMAGE and WORK=$WORK in place."
		return
	fi
	podman rm -f --ignore "$APP_CTR" >/dev/null 2>&1 || true
	[[ -n "$JWKS_PID" ]] && kill "$JWKS_PID" 2>/dev/null || true
	# The image too (pd-5aba).
	podman rmi -f "$IMAGE" >/dev/null 2>&1 || true
	rm -rf "$WORK"
}
trap cleanup EXIT

# The shipped image with the shipped ENTRYPOINT and DUMPSTAR_HOME; the only thing
# added is the env vars that switch resolution on.
start_app() { # start_app [-e VAR=VAL ...]
	podman rm -f --ignore "$APP_CTR" >/dev/null 2>&1 || true
	podman run -d --name "$APP_CTR" -p "127.0.0.1:${PDH_PORT:-}:8080" \
		-v "${DATA}:/data:Z" "$@" "$IMAGE" >/dev/null
	PORT="$(podman inspect -f '{{ (index .NetworkSettings.Ports "8080/tcp" 0).HostPort }}' "$APP_CTR" 2>/dev/null || true)"
	if [[ -z "$PORT" ]]; then
		echo "  ABORT: podman published no host port for ${APP_CTR}."
		podman logs "$APP_CTR" 2>&1 | sed 's/^/  /'
		exit 1
	fi
}
stop_app() { podman rm -f --ignore "$APP_CTR" >/dev/null 2>&1 || true; }

APP_ANSWERING=0
app_settled() {
	if curl -sf -o /dev/null "http://127.0.0.1:${PORT}/health"; then
		APP_ANSWERING=1
		return 0
	fi
	[[ "$(podman inspect -f '{{.State.Status}}' "$APP_CTR" 2>/dev/null)" == "exited" ]]
}
wait_up() {
	APP_ANSWERING=0
	wait_until 45 0.25 app_settled || true
	[[ $APP_ANSWERING == 1 ]] && echo up || echo down
}

app_state() { podman inspect -f '{{.State.Status}}/{{.State.ExitCode}}' "$APP_CTR" 2>/dev/null || echo missing; }

require_up() { # require_up <label>
	local up
	up="$(wait_up)"
	check "$1" up "$up"
	if [[ "$up" == up ]]; then
		return
	fi
	echo
	echo "  ABORT: the server never answered on 127.0.0.1:${PORT} — everything"
	echo "         after this asserts over HTTP and would report 000, not a cause."
	echo "  container: ${APP_CTR}  state: $(app_state)"
	echo "  --- podman logs ${APP_CTR} ---"
	podman logs "$APP_CTR" 2>&1 | sed 's/^/  /'
	echo "  --- end of log ---"
	echo "  ${pass} passed, ${fail} failed before the abort"
	exit 1
}

dumpstar() { podman run --rm -v "${DATA}:/data:Z" --entrypoint dumpstar "$IMAGE" "$@"; }

# The Access JWT, passed in the header Cloudflare injects on proxied requests.
# In a real deployment the browser carries a CF_Authorization cookie; in this
# test we use the header form which the server accepts equally.
api_status() { # api_status <jwt>
	curl -s -o /dev/null -w '%{http_code}' \
		-H "Cf-Access-Jwt-Assertion: $1" \
		"http://127.0.0.1:${PORT}/api/collection"
}
api_status_anonymous() {
	curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/api/collection"
}

tenant_files() { ls "${DATA}/tenants" 2>/dev/null | grep '\.sqlite$' | sort; }

log "1. the shipped image"
dumpstar_image_ensure "$IMAGE" "$REPO_DIR" >/dev/null
echo "  $IMAGE"

log "2. a data directory provisioned with two users"
mkdir -p "$DATA"
cp "${FIXTURES}/shared.sqlite" "${DATA}/shared.sqlite"
dumpstar tenant create alice >/dev/null
dumpstar tenant create bob >/dev/null
dumpstar tenant detach bob --yes >/dev/null
# An unregistered database in tenants/ — the registry is the only lookup path,
# so a file with no row cannot be reached regardless of its name.
cp "${FIXTURES}/collection.sqlite" "${DATA}/tenants/ghost.sqlite"
check "one registered active user" "1" \
	"$(sqlite3 "file:${DATA}/registry.sqlite?mode=ro" \
		"SELECT count(*) FROM user WHERE state = 'active' AND handle = 'alice';")"
check "bob's row survives, detached" "detached" \
	"$(sqlite3 "file:${DATA}/registry.sqlite?mode=ro" \
		"SELECT state FROM user WHERE handle = 'bob';")"
BEFORE_FILES="$(tenant_files)"

log "3. JWKS server started; alice's email bound in the registry"
# tests/lib/test_jwks.py generates a fresh RSA-2048 key pair, writes its JWKS
# to JWKS_DIR/jwks.json, and serves it at /cdn-cgi/access/certs on a random
# port. It binds to 0.0.0.0 so the app container can reach it via
# host.containers.internal (set by Podman in the container's /etc/hosts).
mkdir -p "$JWKS_DIR"
python3 "${REPO_DIR}/tests/lib/test_jwks.py" serve "$JWKS_DIR" &
JWKS_PID=$!
for i in $(seq 1 40); do
	[[ -f "$JWKS_DIR/jwks.json" ]] && break
	sleep 0.1
done
check "JWKS server is ready" "1" "$([[ -f "$JWKS_DIR/jwks.json" ]] && echo 1 || echo 0)"
JWKS_PORT=$(jq -r '.port' "$JWKS_DIR/jwks.json")
JWKS_AUD=$(jq -r '.aud' "$JWKS_DIR/jwks.json")
JWKS_ISSUER=$(jq -r '.issuer' "$JWKS_DIR/jwks.json")
# The URL the container uses: host.containers.internal is Podman's name for the
# host machine, present in every container's /etc/hosts.
JWKS_URL="http://host.containers.internal:${JWKS_PORT}/cdn-cgi/access/certs"

# Bind alice's email — this is the runtime step that connects an authenticated
# identity to a tenant. Without it, alice's JWT would get a 403.
dumpstar tenant identity add alice --email alice@example.com >/dev/null
check "alice's email is bound" "1" \
	"$(sqlite3 "file:${DATA}/registry.sqlite?mode=ro" \
		"SELECT count(*) FROM user_identity ui JOIN user u USING (database_id)
		 WHERE u.handle='alice' AND ui.email='alice@example.com';")"

# Issue test tokens. ALICE_TOKEN is signed by the test JWKS key and carries
# alice@example.com; UNBOUND_TOKEN carries an email nobody is registered under.
ALICE_TOKEN=$(python3 "${REPO_DIR}/tests/lib/test_jwks.py" issue "$JWKS_DIR" alice@example.com)
UNBOUND_TOKEN=$(python3 "${REPO_DIR}/tests/lib/test_jwks.py" issue "$JWKS_DIR" unbound@example.com)

log "4. WITHOUT Access vars the server refuses to start"
# access::AccessConfig::from_env requires DUMPSTAR_ACCESS_TEAM_DOMAIN,
# DUMPSTAR_ACCESS_AUD and DUMPSTAR_ACCESS_JWKS_URL at startup. Without them the
# server exits immediately — §5 onwards can only be reached by testing AGAINST
# this guard, not by removing it. There is no single-tenant mode and no switch
# to fall back to.
start_app
check "it never listens" "down" "$(wait_up)"
check "the process exited non-zero" "exited/1" "$(app_state)"
# ANCHOR ON THE REFUSAL LINE, NOT THE PHRASE: the message names Access more than
# once, so a bare `grep -c 'Cloudflare Access'` would score a correct refusal 2.
check "and said that Cloudflare Access was missing" "1" \
	"$(podman logs "$APP_CTR" 2>&1 | grep -c 'refusing to start: Cloudflare Access is not configured' || true)"
stop_app
# A partial configuration is a refusal too, and names what is missing.
start_app -e DUMPSTAR_ACCESS_TEAM_DOMAIN="$JWKS_ISSUER" -e DUMPSTAR_ACCESS_AUD="$JWKS_AUD"
check "a partial config never listens" "down" "$(wait_up)"
check "and names the missing JWKS url" "1" \
	"$(podman logs "$APP_CTR" 2>&1 | grep -c 'DUMPSTAR_ACCESS_JWKS_URL not set' || true)"
stop_app

log "5. with Access configured: a bound email JWT is served"
# The three Access vars configure the JWT verifier. JWKS_URL points to our
# test JWKS server, which the container reaches via host.containers.internal.
start_app \
	-e DUMPSTAR_ACCESS_TEAM_DOMAIN="$JWKS_ISSUER" \
	-e DUMPSTAR_ACCESS_AUD="$JWKS_AUD" \
	-e DUMPSTAR_ACCESS_JWKS_URL="$JWKS_URL"
require_up "the server came up with Access configured"
check "alice's JWT is served" "200" "$(api_status "$ALICE_TOKEN")"

log "6. a valid JWT for an UNBOUND email is a 403"
# The JWT is correctly signed and not expired; the verifier accepts it. But
# the email (unbound@example.com) has no binding in the registry, so the
# resolver answers 403 and names the command that fixes it.
check "unbound email -> 403" "403" "$(api_status "$UNBOUND_TOKEN")"
# Verify the 403 names the fix command (not just a generic error).
UNBOUND_BODY=$(curl -s \
	-H "Cf-Access-Jwt-Assertion: $UNBOUND_TOKEN" \
	"http://127.0.0.1:${PORT}/api/collection" || true)
check "the 403 names the fix" "1" \
	"$(printf '%s' "$UNBOUND_BODY" | grep -c 'dumpstar tenant identity add' || true)"

log "7. an INVALID JWT is a 401"
# The server rejects the token before it ever reaches the tenant resolver.
check "invalid JWT -> 401" "401" "$(api_status not.a.valid.jwt)"

log "8. no JWT at all is a 401 — there is no ambient identity"
check "anonymous -> 401" "401" "$(api_status_anonymous)"

log "9. nothing above provisioned anything new"
check "the tenant databases are exactly the ones we made" "$BEFORE_FILES" \
	"$(tenant_files)"
check "and nothing was created beside the catalog" "absent" \
	"$([ -e "${DATA}/collection.sqlite" ] && echo present || echo absent)"

log "10. detach removes access, reattach restores it, purge is guarded"
ALICE_ID=$(sqlite3 "file:${DATA}/registry.sqlite?mode=ro" \
	"SELECT database_id FROM user WHERE handle='alice';")
# purge refuses a live row and a handle; nothing is removed by either.
check "purge refuses a live user" "1" \
	"$(dumpstar tenant purge "$ALICE_ID" --yes 2>&1 | grep -c 'still active' || true)"
check "purge refuses a handle" "nonzero" \
	"$(dumpstar tenant purge alice --yes >/dev/null 2>&1 && echo zero || echo nonzero)"
check "alice's database is still there" "1" "$(tenant_files | grep -c "^${ALICE_ID}.sqlite$" || true)"
dumpstar tenant detach alice --yes >/dev/null
check "detached: alice's JWT -> 403" "403" "$(api_status "$ALICE_TOKEN")"
check "reattach refuses a handle" "nonzero" \
	"$(dumpstar tenant reattach alice >/dev/null 2>&1 && echo zero || echo nonzero)"
dumpstar tenant reattach "$ALICE_ID" >/dev/null
check "reattached: alice's JWT -> 200" "200" "$(api_status "$ALICE_TOKEN")"
check "reattach provisioned nothing new" "$BEFORE_FILES" "$(tenant_files)"

log "RESULT"
echo "  ${pass} passed, ${fail} failed"
[[ $fail -eq 0 ]] || {
	echo "  --- last 30 lines from ${APP_CTR} ---"
	podman logs "$APP_CTR" 2>&1 | tail -30 | sed 's/^/  /'
	exit 1
}
echo "  PASS — Access config required to start, JWT verified against JWKS,"
echo "         bound email is served, unbound is 403, invalid or absent is 401,"
echo "         and reattach restores what detach removed."
