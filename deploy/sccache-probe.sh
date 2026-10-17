#!/bin/sh
# sccache-probe.sh <sccache> — exit 0 when the sccache server starts and wraps
# rustc, else print why on stdout and exit 1. The image build treats sccache as
# an optimisation: a server that times out (its backend unreachable) must cost
# the cache, never the build (sp-ma66i8).
sc="$1"
SCCACHE_STARTUP_TIMEOUT="${SCCACHE_STARTUP_TIMEOUT:-20}"
export SCCACHE_STARTUP_TIMEOUT
if out="$(timeout "${DUMPSTAR_SCCACHE_PROBE_SECS:-60}" "$sc" rustc -vV 2>&1)"; then
	exit 0
fi
echo "the sccache server did not start: $(printf '%s' "$out" | head -1)"
"$sc" --stop-server >/dev/null 2>&1 || true
exit 1
