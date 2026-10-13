#!/usr/bin/env bash
#
# Seed Dumpstar catalog data. Two modes:
#
#   Per-instance seed (default) — populate an existing instance's data volume
#   by running `dumpstar setup` in a one-off container:
#       bash deploy/seed.sh <instance> [extra dumpstar setup args...]
#   Examples:
#       bash deploy/seed.sh prod                  # full catalog build
#       bash deploy/seed.sh prod --skip-tail      # skip the API tail fetch
#
#   Seed-volume mode (--volume) — build a reusable `dumpstar-seed-data` volume
#   once, so future `setup.sh --init` clones it in seconds instead of
#   re-downloading the catalog:
#       bash deploy/seed.sh --volume [--force] [extra dumpstar setup args...]
#   `--force` recreates the volume if it already exists.
#
set -euo pipefail

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SEED_VOLUME="dumpstar-seed-data"

# --- Parse arguments --------------------------------------------------------

VOLUME_MODE=false
FORCE=false
POSITIONAL=()
for arg in "$@"; do
    case $arg in
        --volume) VOLUME_MODE=true ;;
        --force)  FORCE=true ;;
        *) POSITIONAL+=("$arg") ;;
    esac
done

if ! command -v podman >/dev/null 2>&1; then
    echo "ERROR: podman not found. Install it: sudo apt install podman"
    exit 1
fi

# Seed into the store the instance lives in (pd-fite). No-op for prod, which
# never opts in to an alternate store.
# shellcheck source=deploy/store-lib.sh
. "$SCRIPT_DIR/store-lib.sh"
if [ ${#POSITIONAL[@]} -ge 1 ]; then
    dumpstar_store_adopt_instance "${POSITIONAL[0]}"
fi
dumpstar_store_activate

# ============================================================================
# Seed-volume mode — build the reusable dumpstar-seed-data volume.
# ============================================================================

if [ "$VOLUME_MODE" = true ]; then
    if podman volume exists "$SEED_VOLUME" 2>/dev/null; then
        if [ "$FORCE" = true ]; then
            echo "==> Removing existing seed volume..."
            podman volume rm "$SEED_VOLUME"
        else
            echo "Seed volume '$SEED_VOLUME' already exists."
            echo "Use --force to recreate it."
            exit 0
        fi
    fi

    echo "==> Building image dumpstar:latest..."
    # Through the helper for the same two reasons deploy.sh is (pd-h3wy):
    # the previous build's orphaned layers get collected, and the cargo target
    # cache is scoped to this checkout rather than shared as `unscoped`.
    # shellcheck source=deploy/image-lib.sh
    . "$SCRIPT_DIR/image-lib.sh"
    dumpstar_image_ensure dumpstar:latest "$REPO_DIR"

    echo "==> Creating seed volume ($SEED_VOLUME) — runs 'dumpstar setup'..."
    podman volume create "$SEED_VOLUME" >/dev/null
    podman run --rm \
        -v "${SEED_VOLUME}:/data:Z" \
        -e DUMPSTAR_HOME=/data \
        --entrypoint dumpstar \
        localhost/dumpstar:latest \
        setup "${POSITIONAL[@]}"

    echo ""
    echo "==> Seed volume ready."
    echo "    New instances using 'setup.sh --init' clone from it (~seconds)."
    echo "    Recreate with: bash deploy/seed.sh --volume --force"
    exit 0
fi

# ============================================================================
# Per-instance seed — populate one instance's data volume in place.
# ============================================================================

if [ ${#POSITIONAL[@]} -lt 1 ]; then
    echo "Usage: bash deploy/seed.sh <instance> [extra dumpstar setup args...]"
    echo "       bash deploy/seed.sh --volume [--force]   # build reusable seed volume"
    exit 1
fi

INSTANCE="${POSITIONAL[0]}"
EXTRA_ARGS=("${POSITIONAL[@]:1}")
VOLUME="dumpstar-${INSTANCE}-data"
IMAGE="localhost/dumpstar:${INSTANCE}"

if ! podman image exists "$IMAGE" 2>/dev/null; then
    echo "ERROR: image $IMAGE not found — run deploy/setup.sh ${INSTANCE} first."
    exit 1
fi

podman volume exists "$VOLUME" 2>/dev/null || podman volume create "$VOLUME" >/dev/null

echo "==> Running 'dumpstar setup' against ${VOLUME} (downloads the catalog)..."
podman run --rm \
    -v "${VOLUME}:/data:Z" \
    -e DUMPSTAR_HOME=/data \
    --entrypoint dumpstar \
    "$IMAGE" \
    setup "${EXTRA_ARGS[@]}"

echo "==> Catalog populated. Restart: systemctl --user restart dumpstar-${INSTANCE}"
