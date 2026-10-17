#!/usr/bin/env bash
# tier: T1
# covers: deploy/published-image-lib.sh
# tests/ci/published_image_test.sh - deploy/published-image-lib.sh (sp-jooqc0):
# the deploy pulls CI's image keyed by tree, falls back to a build only in `auto`,
# and `pull` never builds. Hermetic: podman is a stub.
set -euo pipefail
cd "$(dirname "$0")/../.."
fail=0
check() { if [ "$2" = "$3" ]; then echo "    ok   $1"; else echo "    FAIL $1: want '$2' got '$3'"; fail=1; fi; }

scratch="$(mktemp -d)"; trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/bin"
cat > "$scratch/bin/podman" <<'STUB'
#!/usr/bin/env bash
echo "$*" >> "$STUB_LOG"
case "$1" in
	image) [ "${2:-}" = exists ] && exit 1; echo abcdef0123456789; exit 0 ;;
	pull) [ "${STUB_PULL:-ok}" = ok ] ;;
	*) exit 0 ;;
esac
STUB
chmod +x "$scratch/bin/podman"
export STUB_LOG="$scratch/log"; : > "$STUB_LOG"
git init -q "$scratch/repo"; git -C "$scratch/repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m x
tree="$(git -C "$scratch/repo" rev-parse 'HEAD^{tree}')"
# shellcheck source=deploy/published-image-lib.sh
. deploy/published-image-lib.sh
export PATH="$scratch/bin:$PATH"

check "ref is keyed by tree" "${DUMPSTAR_IMAGE_REPO}:tree-${tree}" "$(dumpstar_published_ref "$scratch/repo")"
check "auto pulls the published image" "${DUMPSTAR_IMAGE_REPO}:tree-${tree}" "$(dumpstar_image_fetch "$scratch/repo" 2>/dev/null)"
export STUB_PULL=no
check "auto builds when nothing is published" "" "$(dumpstar_image_fetch "$scratch/repo" 2>/dev/null)"
if DUMPSTAR_DEPLOY_IMAGE=pull dumpstar_image_fetch "$scratch/repo" >/dev/null 2>&1; then
	echo "    FAIL pull must fail when nothing is published"; fail=1
else echo "    ok   pull fails rather than build"; fi
check "build mode never pulls" "0" "$(: > "$STUB_LOG"; DUMPSTAR_DEPLOY_IMAGE=build dumpstar_image_fetch "$scratch/repo" 2>/dev/null; grep -c pull "$STUB_LOG" || true)"
[ "$fail" = 0 ] && echo "published image: all assertions passed" || { echo "published image: FAILED"; exit 1; }
