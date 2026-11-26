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

# sp-p7xf0n: the testenv rust tier is a bare container, so it fetches sccache
# alone (runner-deps.sh --sccache). curl is a stub: no network.
mkdir "$stub/bin" "$stub/home"
sccache_only() { # <install dir>
    HOME="$stub/home" DUMPSTAR_SCCACHE_DIR="$1" PATH="$stub/bin:/usr/bin:/bin" \
        bash deploy/runner-deps.sh --sccache 2>&1
}
printf '#!/bin/sh\nexit 22\n' >"$stub/bin/curl"
chmod +x "$stub/bin/curl"
out="$(sccache_only "$stub/failed")"; rc=$?
[ "$rc" -eq 0 ] && [[ "$out" == *"could not install sccache"* && "$out" != *apt* ]] \
    && ok "--sccache: a failed download is a note, nothing else is installed" \
    || bad "--sccache with a failed download: rc=$rc $out"

release=sccache-v0.10.0-x86_64-unknown-linux-musl
mkdir -p "$stub/release/$release"
printf '#!/bin/sh\n' >"$stub/release/$release/sccache"
chmod +x "$stub/release/$release/sccache"
tar -czf "$stub/release.tgz" -C "$stub/release" "$release"
cat >"$stub/bin/curl" <<STUB
#!/bin/sh
while [ \$# -gt 0 ]; do
    [ "\$1" = -o ] && exec cp "$stub/release.tgz" "\$2"
    shift
done
exit 2
STUB
out="$(sccache_only "$stub/installed")"; rc=$?
[ "$rc" -eq 0 ] && [ -x "$stub/installed/sccache" ] \
    && ok "--sccache installs the release's binary" || bad "--sccache did not install: rc=$rc $out"

argv="$(sed -n '/^\[tier\.rust\]$/,/^$/s/^argv = //p' .spira/testenv.toml)"
case "$argv" in
    *"runner-deps.sh --sccache && "*"deploy/ci.sh"*) ok "testenv rust tier fetches sccache before ci.sh" ;;
    *) bad "testenv rust tier does not fetch sccache before ci.sh: $argv" ;;
esac

exit $fail
