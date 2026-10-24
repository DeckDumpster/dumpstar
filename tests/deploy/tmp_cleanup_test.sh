#!/usr/bin/env bash
# tier: T2
# covers: deploy/published-image-lib.sh infra/edge/run.sh infra/edge-shared/run.sh
# tests/deploy/tmp_cleanup_test.sh — a local image build leaves no temp copy of the tree in $TMPDIR
# whether the build succeeds, fails or is interrupted (sp-7vhbw9), and the terraform wrappers cache
# providers once under a persistent dir instead of downloading them into every working copy.
#
# Hermetic: bash, git, tar.
#
#   bash tests/deploy/tmp_cleanup_test.sh
set -euo pipefail
REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0; fail=0
ok() { echo "  PASS  $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL  $1"; fail=$((fail + 1)); }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (want '$2', got '$3')"; fi; }

LIB="$WORK/lib"; TMPD="$WORK/tmp"; SRC="$WORK/src"
mkdir -p "$LIB" "$TMPD" "$SRC"
cp "$REPO_DIR/deploy/published-image-lib.sh" "$LIB/"
# The stub image-lib: behaviour chosen by $STUB_MODE.
cat >"$LIB/image-lib.sh" <<'STUB'
dumpstar_image_ensure() {
	case "$STUB_MODE" in
	ok) return 0 ;;
	fail) return 1 ;;
	hang) sleep 3 & wait ;;
	esac
}
STUB
git -C "$SRC" init -q
echo x >"$SRC/f"
git -C "$SRC" add f
git -C "$SRC" -c user.name=t -c user.email=t@t commit -qm init
tree="$(git -C "$SRC" rev-parse 'HEAD^{tree}')"

run_build() {
	STUB_MODE="$1" TMPDIR="$TMPD" bash -c '
		. "$1/published-image-lib.sh" 2>/dev/null || true
		dumpstar_local_image_build "$2" "$3"' _ "$LIB" "$SRC" "$tree"
}
left() { find "$TMPD" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' '; }

run_build ok >/dev/null 2>&1 || true
check "a successful build leaves no temp copy" 0 "$(left)"
run_build fail >/dev/null 2>&1 || true
check "a failed build leaves no temp copy" 0 "$(left)"

STUB_MODE=hang TMPDIR="$TMPD" setsid bash -c '
	. "$1/published-image-lib.sh" 2>/dev/null || true
	dumpstar_local_image_build "$2" "$3"' _ "$LIB" "$SRC" "$tree" >/dev/null 2>&1 &
pid=$!
for _ in $(seq 1 50); do [ "$(left)" != 0 ] && break; sleep 0.1; done
[ "$(left)" != 0 ] || bad "the interrupted build never made its temp copy"
kill -TERM -- "-$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
sleep 0.5
check "an interrupted build leaves no temp copy" 0 "$(left)"

for r in infra/edge/run.sh infra/edge-shared/run.sh; do
	grep -q 'TF_PLUGIN_CACHE_DIR' "$REPO_DIR/$r" && ok "$r caches providers" || bad "$r sets no TF_PLUGIN_CACHE_DIR"
done

echo "tmp_cleanup_test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
