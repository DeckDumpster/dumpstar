#!/usr/bin/env bash
# The test Cloudflare Access identity provider, for harnesses that start the
# shipped image. Sourced; defines functions and starts nothing.
#
# The server verifies RS256 JWTs against a JWKS (crates/dumpstar-server/src/
# access.rs) and has no bypass, so a harness that starts it without Access
# configured is starting something production never runs. These functions give
# every container-tier harness the same real path tests/tenants/handles.sh uses:
# tests/lib/test-jwks.sh serves a fresh key's JWKS on the host and signs tokens.
#
#   access_start <dir>             serve a JWKS from <dir> in the background;
#                                  sets ACCESS_PID, ACCESS_URL, ACCESS_ISSUER,
#                                  ACCESS_AUD
#   access_env_args                the `-e` words that point a container at it
#   access_env_file <dir> <file>   the same as an EnvironmentFile
#   access_token <dir> <email>     a signed token for <email>
#   access_stop                    stop the server access_start launched
#   access_unit_start <dir> <prefix> [bin]
#                                  serve a JWKS from <dir> as a transient user
#                                  unit that outlives the caller; the unit name is
#                                  <prefix>-<pid>-<random> (never shared between
#                                  runs), recorded in <dir>/../test-jwks.unit and
#                                  set as ACCESS_UNIT
#   access_unit_stop <dir>         stop the unit recorded for <dir>
#
# Containers reach the host as host.containers.internal (Podman's /etc/hosts).

ACCESS_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACCESS_PID=""
ACCESS_URL=""
ACCESS_ISSUER=""
ACCESS_AUD=""

# Read the metadata test-jwks.sh wrote, and derive the container-facing URL.
access_load() { # access_load <dir>
	local meta="$1/jwks.json" port
	port="$(jq -r '.port' "$meta")"
	ACCESS_AUD="$(jq -r '.aud' "$meta")"
	ACCESS_ISSUER="$(jq -r '.issuer' "$meta")"
	ACCESS_URL="http://host.containers.internal:${port}/cdn-cgi/access/certs"
}

# Say why a JWKS server that wrote nothing did not: its unit's state and journal.
access_unit_explain() { # access_unit_explain <unit>
	systemctl --user status "$1" --no-pager 2>&1 | head -n 30 || true
	journalctl --user -u "$1" -n 30 --no-pager 2>&1 || true
}

# Wait up to 30 s (0.1 s polls) for <dir>/jwks.json. Returns 1 on timeout.
access_wait_jwks() { # access_wait_jwks <dir> [unit-or-pid-check-command...]
	local dir="$1" i
	shift
	for i in $(seq 1 300); do
		[[ -f "$dir/jwks.json" ]] && return 0
		"$@" || return 1
		sleep 0.1
	done
	[[ -f "$dir/jwks.json" ]]
}

access_start() { # access_start <dir>
	mkdir -p "$1"
	rm -f "$1/jwks.json"
	bash "${ACCESS_LIB_DIR}/test-jwks.sh" serve "$1" >/dev/null &
	ACCESS_PID=$!
	if ! access_wait_jwks "$1" kill -0 "$ACCESS_PID" 2>/dev/null; then
		if kill -0 "$ACCESS_PID" 2>/dev/null; then
			echo "  ABORT: the test JWKS server (pid $ACCESS_PID) wrote no $1/jwks.json within 30 s."
		else
			echo "  ABORT: the test JWKS server (pid $ACCESS_PID) exited before writing $1/jwks.json."
		fi
		return 1
	fi
	access_load "$1"
}

access_unit_start() { # access_unit_start <dir> <prefix> [bin]
	local dir="$1" prefix="$2" bin="${3:-}" rec="$1/../test-jwks.unit" out rc old
	mkdir -p "$dir"
	if [[ -f "$rec" ]]; then
		old="$(cat "$rec")"
		systemctl --user stop "$old" 2>/dev/null || true
		systemctl --user reset-failed "$old" 2>/dev/null || true
	fi
	# A unit of the retired fixed name may still linger, failed or running.
	systemctl --user stop "$prefix" 2>/dev/null || true
	systemctl --user reset-failed "$prefix" 2>/dev/null || true
	rm -f "$dir/jwks.json"
	ACCESS_UNIT="${prefix}-$$-${RANDOM}${RANDOM}"
	systemctl --user stop "$ACCESS_UNIT" 2>/dev/null || true
	systemctl --user reset-failed "$ACCESS_UNIT" 2>/dev/null || true
	set +e
	out="$(systemd-run --user --quiet --unit "$ACCESS_UNIT" \
		${bin:+--setenv=DUMPSTAR_TEST_JWKS_BIN="$bin"} \
		bash "${ACCESS_LIB_DIR}/test-jwks.sh" serve "$dir" 2>&1)"
	rc=$?
	set -e
	if [[ $rc -ne 0 ]]; then
		echo "ERROR: could not start the test JWKS unit ${ACCESS_UNIT}: ${out}" >&2
		return 1
	fi
	printf '%s\n' "$ACCESS_UNIT" >"$rec"
	if ! access_wait_jwks "$dir" systemctl --user is-active --quiet "$ACCESS_UNIT"; then
		echo "ERROR: the test JWKS unit ${ACCESS_UNIT} wrote no $dir/jwks.json (within 30 s, or it exited):" >&2
		access_unit_explain "$ACCESS_UNIT" >&2
		return 1
	fi
}

access_unit_stop() { # access_unit_stop <dir>
	local rec="$1/../test-jwks.unit"
	[[ -f "$rec" ]] || return 0
	systemctl --user stop "$(cat "$rec")" 2>/dev/null || true
	systemctl --user reset-failed "$(cat "$rec")" 2>/dev/null || true
}

access_env_args() {
	printf '%s\n' -e "DUMPSTAR_ACCESS_TEAM_DOMAIN=${ACCESS_ISSUER}" \
		-e "DUMPSTAR_ACCESS_AUD=${ACCESS_AUD}" \
		-e "DUMPSTAR_ACCESS_JWKS_URL=${ACCESS_URL}"
}

access_env_file() { # access_env_file <dir> <file>
	access_load "$1"
	cat >"$2" <<ENVEOF
# Test Access config written by tests/lib/access.sh — a throwaway JWKS server, not Cloudflare.
DUMPSTAR_ACCESS_TEAM_DOMAIN=${ACCESS_ISSUER}
DUMPSTAR_ACCESS_AUD=${ACCESS_AUD}
DUMPSTAR_ACCESS_JWKS_URL=${ACCESS_URL}
ENVEOF
	chmod 600 "$2"
}

access_token() { # access_token <dir> <email>
	bash "${ACCESS_LIB_DIR}/test-jwks.sh" issue "$1" "$2"
}

access_stop() {
	[[ -n "$ACCESS_PID" ]] && kill "$ACCESS_PID" 2>/dev/null || true
	ACCESS_PID=""
}
