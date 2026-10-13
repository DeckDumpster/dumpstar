#!/usr/bin/env bash
#
# Stop and remove a Dumpstar instance.
# With --purge, also deletes the data volume (the catalog + collection).
#
# Usage:
#   bash deploy/teardown.sh <instance> [--purge]
#
set -euo pipefail

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

PURGE=false
POSITIONAL=()
for arg in "$@"; do
    case $arg in
        --purge) PURGE=true ;;
        *) POSITIONAL+=("$arg") ;;
    esac
done

if [ ${#POSITIONAL[@]} -lt 1 ]; then
    echo "Usage: bash deploy/teardown.sh <instance> [--purge]"
    exit 1
fi

INSTANCE="${POSITIONAL[0]}"
SERVICE_NAME="dumpstar-${INSTANCE}"
QUADLET_FILE="$HOME/.config/containers/systemd/${SERVICE_NAME}.container"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Remove from the store the instance was CREATED in, not whichever one this
# shell happens to point at — otherwise a throwaway instance's image and volume
# leak into a store nothing cleans. The unit file records it (pd-fite).
# shellcheck source=deploy/store-lib.sh
. "$SCRIPT_DIR/store-lib.sh"
dumpstar_store_adopt_instance "$INSTANCE"
dumpstar_store_activate

echo "==> Stopping ${SERVICE_NAME}..."
systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true

# Stop and disable the per-instance refresh + backup-check timers (ivq), the
# offline catalog derive (pd-1uem), the nightly catalog.prices build (pd-up36),
# the transform tier's nightly run (pd-8m5c) and the ownership shipment
# (pd-dxn3) — an instance that is gone must not leave a timer behind still
# firing at its volume, its bucket and its lake
# network.
systemctl --user disable --now "dumpstar-refresh@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-backup-check@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-derive@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-prices@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-catalog-sync@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-catalog-check@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-nessie-backup@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-nessie-backup-check@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-value-snapshots@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-ship@${INSTANCE}.timer" 2>/dev/null || true
systemctl --user disable --now "dumpstar-heartbeat@${INSTANCE}.timer" 2>/dev/null || true

# Stop the Litestream backup sidecar (dumpstar-8ch.3).
systemctl --user stop "dumpstar-litestream-${INSTANCE}.service" 2>/dev/null || true

rm -f "$QUADLET_FILE" \
      "$HOME/.config/containers/systemd/dumpstar-litestream-${INSTANCE}.container"
systemctl --user daemon-reload

# Clear this instance's failed state (pd-n0lf). A unit stopped as part of a
# teardown is not a failure, but systemd keeps it in `--failed` forever, and once
# the unit file is gone it lingers as an unclearable `not-found` ghost. 125 had
# accumulated by 2026-08-13, 61 of them ghosts, which is enough noise to hide a
# real failure in the same listing.
for _u in "$SERVICE_NAME" \
          "dumpstar-litestream-${INSTANCE}.service" \
          "dumpstar-refresh@${INSTANCE}."{service,timer} \
          "dumpstar-backup-check@${INSTANCE}."{service,timer} \
          "dumpstar-derive@${INSTANCE}."{service,timer} \
          "dumpstar-prices@${INSTANCE}."{service,timer} \
          "dumpstar-catalog-sync@${INSTANCE}."{service,timer} \
          "dumpstar-catalog-check@${INSTANCE}."{service,timer} \
          "dumpstar-nessie-backup@${INSTANCE}."{service,timer} \
          "dumpstar-nessie-backup-check@${INSTANCE}."{service,timer} \
          "dumpstar-value-snapshots@${INSTANCE}."{service,timer} \
          "dumpstar-ship@${INSTANCE}."{service,timer} \
          "dumpstar-heartbeat@${INSTANCE}."{service,timer}; do
    systemctl --user reset-failed "$_u" 2>/dev/null || true
done

podman rmi "dumpstar:${INSTANCE}" 2>/dev/null || true

if [ "$PURGE" = true ]; then
    podman volume rm "dumpstar-${INSTANCE}-data" 2>/dev/null || true
    # Litestream config + AWS creds for this instance (holds secrets).
    rm -rf "$HOME/.config/dumpstar/${INSTANCE}"
    echo "==> ${INSTANCE} removed (data volume + backup config purged)."
else
    echo "==> ${INSTANCE} removed. Data volume kept — add --purge to delete it."
fi
