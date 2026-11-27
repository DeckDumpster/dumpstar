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
#   dumpstar_rust_cache_buildargs <dir>  podman build args that hand the image the
#                                same cache; copies the context into <dir>/store
#
# sccache is pinned by flake.nix/flake.lock and installed by
# deploy/runner-deps.sh as nix build's out-link, DUMPSTAR_SCCACHE_DIR. That is
# the only binary used; one on PATH is ignored.
#
# DUMPSTAR_SCCACHE_ENDPOINT   the WebDAV store (default: the tailnet address below)
# DUMPSTAR_SCCACHE_SERVER_PORT  the local sccache server's port (default 4226)
# DUMPSTAR_RUST_CACHE=on      (default) a missing binary, an unreachable store, or
#                             a server that does not start or is gone by a report
#                             compiles cold, slower, and the line printed says so
# DUMPSTAR_RUST_CACHE=required  the same print the same line and return 1
#                             (the testenv rust tier, which cannot finish cold).
#                             SCCACHE_IGNORE_SERVER_IO_ERROR, set under `on`, is
#                             left unset, so a server I/O error fails the compile.
#                             Not every loss does: sccache compiles locally if the
#                             connection just closes mid-compile, and a later
#                             compile starts a new server. The report's check on
#                             enable's server is what turns those red.
# DUMPSTAR_RUST_CACHE=off     compile cold, deliberately
#
# A clean cache is never a failure. Silence would read as "cache working".

DUMPSTAR_SCCACHE_DEFAULT_ENDPOINT="http://100.113.203.21:9431"
DUMPSTAR_SCCACHE_DIR="${DUMPSTAR_SCCACHE_DIR:-$HOME/.cache/dumpstar-sccache-bin}"
DUMPSTAR_SCCACHE_SERVER_PORT="${DUMPSTAR_SCCACHE_SERVER_PORT:-4226}"
DUMPSTAR_RUST_CACHE_STATE=off
# The server is a child of the shell that enabled the cache: its PID is its
# identity, and `kill -0` on it the liveness check (sccache's client falls back
# to compiling uncached, and --show-stats succeeds with no server at all).
DUMPSTAR_SCCACHE_SERVER_PID=
# One log, truncated by each enable.
DUMPSTAR_SCCACHE_SERVER_LOG="${TMPDIR:-/tmp}/dumpstar-sccache-server.log"

# What an unavailable cache costs: nothing but a cold compile, unless required.
_dumpstar_rust_cache_unavailable() {
	[ "${DUMPSTAR_RUST_CACHE:-on}" = required ] || return 0
	echo "==> rust cache: DUMPSTAR_RUST_CACHE=required — refusing to go on without the cache"
	return 1
}

# Start the server in the foreground of a background child, and return 0 once
# it is alive and its port accepts, 1 if it exits or 20s pass first. Its stdio
# is the log alone: a child holding the caller's stdout holds that pipe open.
# It must outlast a long test execution, which sends it nothing, or sccache's
# 10-minute idle exit would read as a lost server; two hours is past any CI run.
# Another holder of the port can answer before this server has failed to bind;
# the report's liveness check is what catches that one.
_dumpstar_sccache_start_server() {
	local bin="$1" deadline=$((SECONDS + 20))
	SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1 SCCACHE_IDLE_TIMEOUT=7200 \
		"$bin" </dev/null >"$DUMPSTAR_SCCACHE_SERVER_LOG" 2>&1 &
	DUMPSTAR_SCCACHE_SERVER_PID=$!
	while kill -0 "$DUMPSTAR_SCCACHE_SERVER_PID" 2>/dev/null; do
		(exec 3<>"/dev/tcp/127.0.0.1/$SCCACHE_SERVER_PORT") 2>/dev/null && return 0
		[ "$SECONDS" -lt "$deadline" ] || break
		sleep 0.1
	done
	kill "$DUMPSTAR_SCCACHE_SERVER_PID" 2>/dev/null || true
	return 1
}

dumpstar_rust_cache_enable() {
	local endpoint="${DUMPSTAR_SCCACHE_ENDPOINT:-${SCCACHE_WEBDAV_ENDPOINT:-$DUMPSTAR_SCCACHE_DEFAULT_ENDPOINT}}"
	local bin="$DUMPSTAR_SCCACHE_DIR/bin/sccache"
	DUMPSTAR_RUST_CACHE_STATE=off
	if [ "${DUMPSTAR_RUST_CACHE:-on}" = off ]; then
		echo "==> rust cache: disabled (DUMPSTAR_RUST_CACHE=off) — compiling cold"
		return 0
	fi
	if [ ! -x "$bin" ]; then
		echo "==> rust cache: sccache not installed at ${bin} (bash deploy/runner-deps.sh --sccache) — compiling cold"
		_dumpstar_rust_cache_unavailable
		return
	fi
	local why
	if ! why="$(curl -sS -m 5 -o /dev/null "$endpoint/" 2>&1)"; then
		echo "==> rust cache: ${endpoint} unreachable — compiling cold (slower, still correct): ${why}"
		_dumpstar_rust_cache_unavailable
		return
	fi
	export SCCACHE_WEBDAV_ENDPOINT="$endpoint"
	export SCCACHE_WEBDAV_KEY_PREFIX="${SCCACHE_WEBDAV_KEY_PREFIX:-/}"
	# Under required, a server I/O error fails the compile (see the mode block).
	[ "${DUMPSTAR_RUST_CACHE:-on}" = required ] || export SCCACHE_IGNORE_SERVER_IO_ERROR=1
	# The server, every client compile and the readiness check use one port.
	export SCCACHE_SERVER_PORT="$DUMPSTAR_SCCACHE_SERVER_PORT"
	"$bin" --stop-server >/dev/null 2>&1 || true
	if ! _dumpstar_sccache_start_server "$bin"; then
		echo "==> rust cache: sccache server did not start on 127.0.0.1:${SCCACHE_SERVER_PORT} — compiling cold"
		cat "$DUMPSTAR_SCCACHE_SERVER_LOG"
		unset RUSTC_WRAPPER
		_dumpstar_rust_cache_unavailable
		return
	fi
	export RUSTC_WRAPPER="$bin"
	# sccache cannot cache incremental artifacts; the test profile defaults to them.
	export CARGO_INCREMENTAL=0
	DUMPSTAR_RUST_CACHE_STATE=on
	"$bin" --zero-stats >/dev/null 2>&1 || true
	echo "==> rust cache: sccache -> ${endpoint}"
}

# dumpstar_rust_cache_report [label]
#
# One line per compile: hits, misses and the hit rate, then the counters are
# zeroed so the next report covers only the compile after it. A rate under 90%
# on a run whose sources did not change is a cache that is not working.
#
# First it asks whether enable's server is still running: if not, the compiles
# since it went were uncached, and under required that returns 1.
dumpstar_rust_cache_report() {
	local label="${1:-host compiles}"
	[ "$DUMPSTAR_RUST_CACHE_STATE" = on ] || { echo "==> rust cache summary (${label}): not in use (cold compile)"; return 0; }
	if ! kill -0 "$DUMPSTAR_SCCACHE_SERVER_PID" 2>/dev/null; then
		echo "==> rust cache summary (${label}): sccache server (pid ${DUMPSTAR_SCCACHE_SERVER_PID}) exited since enable — compiles after that ran uncached"
		cat "$DUMPSTAR_SCCACHE_SERVER_LOG"
		_dumpstar_rust_cache_unavailable || return
		# Cold from here: left set, the wrapper would start an unwatched server.
		DUMPSTAR_RUST_CACHE_STATE=off
		unset RUSTC_WRAPPER
		return 0
	fi
	local out hits misses rate
	out="$("$DUMPSTAR_SCCACHE_DIR/bin/sccache" --show-stats 2>&1)" || true
	hits="$(printf '%s\n' "$out" | awk '/^Cache hits +[0-9]+/ {print $3; exit}')"
	misses="$(printf '%s\n' "$out" | awk '/^Cache misses +[0-9]+/ {print $3; exit}')"
	rate="n/a"
	if [ $(( ${hits:-0} + ${misses:-0} )) -gt 0 ]; then
		rate="$(( 100 * ${hits:-0} / (${hits:-0} + ${misses:-0}) ))%"
	fi
	echo "==> rust cache summary (${label}): hits=${hits:-?} misses=${misses:-?} hit-rate=${rate}"
	if [ "${hits:-0}" = 0 ] && [ "${misses:-0}" != 0 ]; then
		echo "    clean cache: every crate compiled cold this run (slower); the next run reads what this one wrote"
	fi
	"$DUMPSTAR_SCCACHE_DIR/bin/sccache" --zero-stats >/dev/null 2>&1 || true
}

# Build args for `podman build`: sccache's whole /nix/store closure as a named
# build context (a Nix binary runs in the bookworm builder only with its own
# libc beside it), the binary's path in it, and the endpoint. Empty when the
# cache is off, which the Containerfile reads as "compile cold". <dir> is the
# caller's, and so is its removal, including after a failed or interrupted copy.
dumpstar_rust_cache_buildargs() {
	[ "$DUMPSTAR_RUST_CACHE_STATE" = on ] || return 0
	local dir="$1" out closure path
	out="$(readlink -f "$DUMPSTAR_SCCACHE_DIR")"
	if ! closure="$(nix-store --query --requisites "$out")"; then
		echo "==> rust cache: nix-store cannot list the closure of ${out}; the image compiles cold" >&2
		return 1
	fi
	mkdir "$dir/store"
	for path in $closure; do
		if ! cp -a "$path" "$dir/store/"; then
			echo "==> rust cache: cannot copy ${path}; the image compiles cold" >&2
			return 1
		fi
	done
	# The store is read-only; the copy must not be, or nothing can clean it up.
	chmod -R u+w "$dir/store"
	printf '%s\n' "--build-context" "sccache-bin=${dir}" \
		"--build-arg" "SCCACHE_BIN=${out}/bin/sccache" \
		"--build-arg" "SCCACHE_WEBDAV_ENDPOINT=${SCCACHE_WEBDAV_ENDPOINT}"
}
