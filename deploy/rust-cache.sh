#!/usr/bin/env bash
#
# The shared Rust compile cache (sp-2i16qf), sourced by deploy/ci.sh and
# deploy/image-lib.sh.
#
# A CI run is a fresh VM with nothing on it, so every cargo build used to start
# cold: `cargo test` (~10 min) and then the image's release build in the
# Containerfile builder, whose BuildKit cache mounts are empty on a new VM.
# Neither remembered anything from the run before. Both now compile through
# sccache against the one store this host already serves to Spira's builds
# (sccache-dav, reached over the tailnet), so a crate whose sources did not
# change is fetched, not compiled — across runs and across the two builds.
#
#   dumpstar_rust_cache_enable   export RUSTC_WRAPPER + endpoint, or say why not
#   dumpstar_rust_cache_report   print the hit/miss summary
#   dumpstar_rust_cache_buildargs  podman build args that hand the image the same cache
#
# DUMPSTAR_SCCACHE_ENDPOINT   the WebDAV store (default: the tailnet address below)
# DUMPSTAR_RUST_CACHE=off     compile cold, deliberately
#
# A clean or unreachable cache is NOT a failure: the build runs, slower, and
# the line printed here says so. Silence would read as "cache working".

DUMPSTAR_SCCACHE_DEFAULT_ENDPOINT="http://100.113.203.21:9431"
DUMPSTAR_SCCACHE_DIR="${DUMPSTAR_SCCACHE_DIR:-$HOME/.cache/dumpstar-sccache-bin}"
DUMPSTAR_RUST_CACHE_STATE=off

dumpstar_rust_cache_enable() {
	local endpoint="${DUMPSTAR_SCCACHE_ENDPOINT:-${SCCACHE_WEBDAV_ENDPOINT:-$DUMPSTAR_SCCACHE_DEFAULT_ENDPOINT}}"
	DUMPSTAR_RUST_CACHE_STATE=off
	# The static release runner-deps.sh installs wins over any other sccache on
	# PATH: it is the one the image's older-glibc builder can execute.
	[ -x "$DUMPSTAR_SCCACHE_DIR/sccache" ] && PATH="$DUMPSTAR_SCCACHE_DIR:$PATH"
	if [ "${DUMPSTAR_RUST_CACHE:-on}" = off ]; then
		echo "==> rust cache: disabled (DUMPSTAR_RUST_CACHE=off) — compiling cold"
		return 0
	fi
	if ! command -v sccache >/dev/null 2>&1; then
		echo "==> rust cache: sccache not installed (bash deploy/runner-deps.sh) — compiling cold"
		return 0
	fi
	if ! curl -sS -m 5 -o /dev/null "$endpoint/" 2>/dev/null; then
		echo "==> rust cache: ${endpoint} unreachable — compiling cold (slower, still correct)"
		return 0
	fi
	export SCCACHE_WEBDAV_ENDPOINT="$endpoint"
	export SCCACHE_WEBDAV_KEY_PREFIX="${SCCACHE_WEBDAV_KEY_PREFIX:-/}"
	export SCCACHE_IGNORE_SERVER_IO_ERROR=1
	export RUSTC_WRAPPER="$(command -v sccache)"
	# sccache cannot cache incremental artifacts; the test profile defaults to them.
	export CARGO_INCREMENTAL=0
	DUMPSTAR_RUST_CACHE_STATE=on
	sccache --stop-server >/dev/null 2>&1 || true
	sccache --start-server >/dev/null 2>&1 || true
	sccache --zero-stats >/dev/null 2>&1 || true
	echo "==> rust cache: sccache -> ${endpoint}"
}

dumpstar_rust_cache_report() {
	[ "$DUMPSTAR_RUST_CACHE_STATE" = on ] || { echo "==> rust cache summary: not in use (cold compile)"; return 0; }
	local out hits misses
	out="$(sccache --show-stats 2>&1)" || true
	hits="$(printf '%s\n' "$out" | awk '/^Cache hits +[0-9]+/ {print $3; exit}')"
	misses="$(printf '%s\n' "$out" | awk '/^Cache misses +[0-9]+/ {print $3; exit}')"
	echo "==> rust cache summary (host compiles): hits=${hits:-?} misses=${misses:-?}"
	if [ "${hits:-0}" = 0 ] && [ "${misses:-0}" != 0 ]; then
		echo "    clean cache: every crate compiled cold this run (slower); the next run reads what this one wrote"
	fi
}

# Build args for `podman build`: the static sccache binary as a named build
# context, and the endpoint. Empty when the cache is off, which the Containerfile
# reads as "compile cold".
dumpstar_rust_cache_buildargs() {
	[ "$DUMPSTAR_RUST_CACHE_STATE" = on ] || return 0
	local bin dir
	bin="$(command -v sccache)"
	dir="$(mktemp -d "${TMPDIR:-/tmp}/dumpstar-sccache.XXXXXX")"
	cp "$bin" "$dir/sccache"
	# A host binary linked against a libc newer than the builder's would not run
	# there; the Containerfile checks that it executes and compiles cold if not.
	printf '%s\n' "--build-context" "sccache-bin=${dir}" \
		"--build-arg" "SCCACHE_WEBDAV_ENDPOINT=${SCCACHE_WEBDAV_ENDPOINT}"
}
