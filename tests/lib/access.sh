#!/usr/bin/env bash
# The test Cloudflare Access identity provider, for harnesses that start the
# shipped image. Sourced; defines functions and starts nothing.
#
# The server verifies RS256 JWTs against a JWKS (crates/dumpstar-server/src/
# access.rs) and has no bypass, so a harness that starts it without Access
# configured is starting something production never runs. These functions give
# every container-tier harness the same real path tests/tenants/handles.sh uses:
# tests/lib/test_jwks.py serves a fresh key's JWKS on the host and signs tokens.
#
#   access_start <dir>             serve a JWKS from <dir> in the background;
#                                  sets ACCESS_PID, ACCESS_URL, ACCESS_ISSUER,
#                                  ACCESS_AUD
#   access_env_args                the `-e` words that point a container at it
#   access_env_file <dir> <file>   the same as an EnvironmentFile
#   access_token <dir> <email>     a signed token for <email>
#   access_stop                    stop the server access_start launched
#
# Containers reach the host as host.containers.internal (Podman's /etc/hosts).

ACCESS_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACCESS_PID=""
ACCESS_URL=""
ACCESS_ISSUER=""
ACCESS_AUD=""

# Read the metadata test_jwks.py wrote, and derive the container-facing URL.
access_load() { # access_load <dir>
	local meta="$1/jwks.json" port
	port="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['port'])" "$meta")"
	ACCESS_AUD="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['aud'])" "$meta")"
	ACCESS_ISSUER="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['issuer'])" "$meta")"
	ACCESS_URL="http://host.containers.internal:${port}/cdn-cgi/access/certs"
}

access_start() { # access_start <dir>
	mkdir -p "$1"
	rm -f "$1/jwks.json"
	python3 "${ACCESS_LIB_DIR}/test_jwks.py" serve "$1" >/dev/null &
	ACCESS_PID=$!
	local i
	for i in $(seq 1 100); do
		[[ -f "$1/jwks.json" ]] && break
		sleep 0.1
	done
	if [[ ! -f "$1/jwks.json" ]]; then
		echo "  ABORT: the test JWKS server wrote no $1/jwks.json (needs python3 with cryptography and PyJWT)."
		return 1
	fi
	access_load "$1"
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
	python3 "${ACCESS_LIB_DIR}/test_jwks.py" issue "$1" "$2"
}

access_stop() {
	[[ -n "$ACCESS_PID" ]] && kill "$ACCESS_PID" 2>/dev/null || true
	ACCESS_PID=""
}
