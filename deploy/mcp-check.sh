#!/usr/bin/env bash
# mcp-check.sh — prove /mcp is switched on where it is served (sp-tfkp9o).
#
# The feature is gated on DUMPSTAR_MCP_RESOURCE; unset, the server serves no /mcp and the
# browser sign-in gate 303s the protected-resource metadata to the login page, which looks
# like "nothing wrong" to every other check. This asks the public URL the two questions an
# agent client asks first:
#
#   1. GET <base>/.well-known/oauth-protected-resource answers 200 and names the expected
#      resource.
#   2. POST <base>/mcp with no token answers 401 with a WWW-Authenticate header.
#
# Usage: mcp-check.sh [base-url] [expected-resource]
#   defaults: https://star.dumpster.cards and <base>/mcp. Retries for up to
#   DUMPSTAR_MCP_CHECK_WAIT seconds (default 60) while the restarted server comes up.
set -uo pipefail

BASE="${1:-https://star.dumpster.cards}"
RESOURCE="${2:-${BASE}/mcp}"
WAIT="${DUMPSTAR_MCP_CHECK_WAIT:-60}"

probe() {
    local meta code hdrs
    meta="$(curl -s -m 10 -w '\n%{http_code}' "${BASE}/.well-known/oauth-protected-resource")" || return 1
    code="${meta##*$'\n'}"
    [ "$code" = 200 ] || { echo "metadata: HTTP ${code}, want 200" >&2; return 1; }
    case "$meta" in
        *"\"resource\":\"${RESOURCE}\""* | *"\"resource\": \"${RESOURCE}\""*) ;;
        *) echo "metadata does not name resource ${RESOURCE}" >&2; return 1 ;;
    esac
    hdrs="$(curl -s -m 10 -o /dev/null -D - -X POST -H 'Content-Type: application/json' -d '{}' "${BASE}/mcp")" || return 1
    case "$hdrs" in
        HTTP*" 401"*) ;;
        *) echo "POST /mcp without a token: want 401" >&2; return 1 ;;
    esac
    grep -qi '^www-authenticate:' <<<"$hdrs" || { echo "401 lacks WWW-Authenticate" >&2; return 1; }
}

deadline=$((SECONDS + WAIT))
while ! probe; do
    if [ "$SECONDS" -ge "$deadline" ]; then
        echo "mcp-check: ${BASE} does not serve /mcp (is DUMPSTAR_MCP_RESOURCE set?)" >&2
        exit 1
    fi
    sleep 2
done
echo "mcp-check: ${BASE}/mcp is served and advertises ${RESOURCE}"
