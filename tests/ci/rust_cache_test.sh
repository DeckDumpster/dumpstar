#!/usr/bin/env bash
# tier: T0
# covers: deploy/ci.sh Containerfile
# The shared Rust compile cache wiring (sp-2i16qf): hermetic, no cargo, no podman.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
ok()  { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

. deploy/rust-cache.sh

out="$(DUMPSTAR_RUST_CACHE=off dumpstar_rust_cache_enable)"
case "$out" in *"compiling cold"*) ok "disabled cache says so" ;; *) bad "disabled cache is silent: $out" ;; esac

out="$(DUMPSTAR_SCCACHE_DIR=/nonexistent PATH=/usr/bin:/bin dumpstar_rust_cache_enable)"
case "$out" in *"compiling cold"*) ok "missing sccache says so" ;; *) bad "missing sccache is silent: $out" ;; esac

out="$(DUMPSTAR_SCCACHE_ENDPOINT=http://127.0.0.1:1 dumpstar_rust_cache_enable)"
case "$out" in *"compiling cold"*|*"not installed"*) ok "unreachable store falls back, says so" ;; *) bad "unreachable store: $out" ;; esac

out="$(DUMPSTAR_RUST_CACHE_STATE=off dumpstar_rust_cache_report)"
case "$out" in *"not in use"*) ok "report states a cold run" ;; *) bad "report: $out" ;; esac

grep -q 'dumpstar_rust_cache_enable' deploy/ci.sh && grep -q 'dumpstar_rust_cache_buildargs' deploy/image-lib.sh \
    && ok "ci.sh and image-lib.sh use the cache" || bad "cache not wired into ci.sh/image-lib.sh"
grep -q 'RUSTC_WRAPPER' Containerfile && grep -q 'sccache-bin' Containerfile \
    && ok "Containerfile builder consumes the cache" || bad "Containerfile does not use sccache"

# sp-ma66i8: a sccache whose server cannot start costs the cache, not the build.
stub="$(mktemp -d)"; trap 'rm -rf "$stub"' EXIT
printf '#!/bin/sh\necho "sccache: error: Timed out waiting for server startup." >&2\nexit 2\n' >"$stub/sccache"
chmod +x "$stub/sccache"
out="$(sh deploy/sccache-probe.sh "$stub/sccache")"; rc=$?
[ "$rc" -ne 0 ] && case "$out" in *"did not start"*) ok "dead sccache server fails the probe, says why" ;; *) bad "probe silent: $out" ;; esac || bad "probe passed a dead server"
printf '#!/bin/sh\nexec "$@"\n' >"$stub/sccache"
sh deploy/sccache-probe.sh "$stub/sccache" >/dev/null 2>&1 && ok "working sccache passes the probe" || bad "working sccache failed the probe"
grep -q 'sccache-probe.sh' Containerfile && ok "Containerfile probes before enabling the wrapper" || bad "Containerfile does not probe"

exit $fail
