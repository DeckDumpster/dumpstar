#!/usr/bin/env bash
#
# Rebuild the image, reinstall the unit files, and restart a Dumpstar
# instance. Delegates to setup.sh if the instance does not exist yet.
#
# "Reinstall the unit files" is the whole of pd-2t6u: the units under deploy/ are
# templates copied into ~/.config at install time, and until this script started
# re-rendering them, only setup.sh ever did — so a deploy shipped the new binary
# and left the units at whatever version the instance was created with. Prod ran
# a pre-multi-tenant Litestream unit, with no OnFailure= alerting, for months.
#
# The instance's published port is preserved across the refresh (see
# deploy/units-lib.sh) — shipping a unit-file change must not move the address.
#
# Usage:
#   bash deploy/deploy.sh <instance>
#
set -euo pipefail

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

if [ $# -lt 1 ]; then
    echo "Usage: bash deploy/deploy.sh <instance>"
    exit 1
fi

INSTANCE="$1"
SERVICE_NAME="dumpstar-${INSTANCE}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
QUADLET_FILE="$HOME/.config/containers/systemd/${SERVICE_NAME}.container"

# Rebuild into the store the instance already lives in (pd-fite). Prod's unit
# carries no store flags, so prod keeps using Podman's default store.
# shellcheck source=deploy/store-lib.sh
. "$SCRIPT_DIR/store-lib.sh"
dumpstar_store_adopt_instance "$INSTANCE"
dumpstar_store_activate

if [ ! -f "$QUADLET_FILE" ]; then
    echo "==> No instance '$INSTANCE' yet — running setup..."
    bash "$SCRIPT_DIR/setup.sh" "$INSTANCE"
else
    # Config first, before anything is built, installed or restarted (sp-kaj3xv):
    # a release that adds a required key must not restart a running instance into
    # an env file that lacks it. Same scaffold setup.sh uses; a key that has no
    # default stops the deploy here, with the old container still serving.
    # shellcheck source=deploy/config-lib.sh
    . "$SCRIPT_DIR/config-lib.sh"
    dumpstar_scaffold_config "$INSTANCE"
    if ! dumpstar_config_converge "${HOME}/.config/dumpstar/${INSTANCE}"; then
        echo "ERROR: deploy stopped before touching ${SERVICE_NAME}; it is still running its previous version." >&2
        echo "       Fix the config named above in ${HOME}/.config/dumpstar/${INSTANCE}/ and redeploy." >&2
        exit 1
    fi

    # Keep the image that is serving, to put back with the unit (sp-h5nmbc): the
    # previous unit run against the new image crash-looped on a key it lacked.
    PREVIOUS_IMAGE_ID="$(podman image inspect --format '{{.Id}}' "dumpstar:${INSTANCE}" 2>/dev/null || true)"
    if [ -n "$PREVIOUS_IMAGE_ID" ]; then
        podman tag "$PREVIOUS_IMAGE_ID" "dumpstar:${INSTANCE}-prev"
    fi

    echo "==> Rebuilding image dumpstar:${INSTANCE}..."
    # Through the helper, not a bare `podman build` (pd-h3wy). Two things came
    # free with it that this line had been missing for as long as it existed:
    # the build collects what the PREVIOUS build orphaned — prod's own rebuild
    # was the biggest single producer of the ~2 GB of unreachable layers per
    # build that filled / — and it passes CARGO_TARGET_CACHE_SCOPE, so prod
    # stops compiling under the `unscoped` target-cache id it shared with every
    # other unscoped build on the box (pd-sjn7).
    # shellcheck source=deploy/image-lib.sh
    . "$SCRIPT_DIR/image-lib.sh"
    # The image CI built and tested for this tree, pulled; building here is the
    # fallback (sp-jooqc0, deploy/published-image-lib.sh).
    # shellcheck source=deploy/published-image-lib.sh
    . "$SCRIPT_DIR/published-image-lib.sh"
    PUBLISHED="$(dumpstar_image_fetch "$REPO_DIR")" || {
        echo "ERROR: deploy stopped before touching ${SERVICE_NAME}; it is still running its previous version." >&2
        exit 1
    }
    dumpstar_refuse_local_image "$INSTANCE" "$PUBLISHED" "${DUMPSTAR_PREBUILT_IMAGE:-}" || {
        echo "ERROR: deploy stopped before touching ${SERVICE_NAME}; it is still running its previous version." >&2
        exit 1
    }
    if [ -n "$PUBLISHED" ]; then export DUMPSTAR_PREBUILT_IMAGE="$PUBLISHED"; fi
    dumpstar_image_ensure "dumpstar:${INSTANCE}" "$REPO_DIR"
    unset DUMPSTAR_PREBUILT_IMAGE
    podman tag "dumpstar:${INSTANCE}" dumpstar:latest

    # Ship the unit files too, not just the image (pd-2t6u). The units under
    # deploy/ are templates that get copied into ~/.config at install time, so an
    # existing instance kept running whatever copy setup.sh made — for prod, the
    # pre-multi-tenant sidecar unit from before the alarming layers existed. A
    # deploy that updates the binary and leaves the units on last year's version
    # is how the repo came to disagree with the box for months.
    #
    # No port argument: the instance keeps the port it already publishes. Passing
    # nothing here would otherwise mean "let Podman pick", which moves prod off
    # 8090 — refreshing the units must not change the address they serve on.
    # Keep the unit the instance is running, to put back if the new one does not
    # come up (sp-5ekmwm).
    PREVIOUS_UNIT="$(mktemp)"
    cp "$QUADLET_FILE" "$PREVIOUS_UNIT"
    echo "==> Installing unit files from this checkout..."
    # shellcheck source=deploy/units-lib.sh
    . "$SCRIPT_DIR/units-lib.sh"
    dumpstar_units_install "$INSTANCE"
    dumpstar_units_report

fi

echo "==> Reloading systemd and restarting ${SERVICE_NAME}..."
# shellcheck source=deploy/diagnostics-lib.sh
. "$SCRIPT_DIR/diagnostics-lib.sh"
systemctl --user daemon-reload

# A unit that does not come up is rolled back to the one it replaced, so a bad
# option in a new unit costs seconds, not an outage (sp-5ekmwm: a pasta option the
# host's podman rejected left prod at 502 until it was fixed by hand).
dumpstar_unit_serves() {
    local url i
    url="$(dumpstar_origin_url "$INSTANCE")" || return 1
    for i in $(seq 1 "${DUMPSTAR_DEPLOY_HEALTH_TRIES:-30}"); do
        [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$url" 2>/dev/null)" = "200" ] && return 0
        sleep "${DUMPSTAR_DEPLOY_HEALTH_SLEEP:-2}"
    done
    return 1
}
DRAIN_SINCE="$(date '+%Y-%m-%d %H:%M:%S')"
if ! { systemctl --user restart "$SERVICE_NAME" && dumpstar_unit_serves; }; then
    dump_unit_diagnostics "$SERVICE_NAME"
    if [ -n "${PREVIOUS_UNIT:-}" ] && [ -s "$PREVIOUS_UNIT" ]; then
        echo "ERROR: ${SERVICE_NAME} did not come up healthy; restoring the previous unit and image." >&2
        cp "$PREVIOUS_UNIT" "$QUADLET_FILE"
        if [ -n "${PREVIOUS_IMAGE_ID:-}" ]; then
            podman tag "dumpstar:${INSTANCE}-prev" "dumpstar:${INSTANCE}" || true
            podman tag "dumpstar:${INSTANCE}-prev" dumpstar:latest || true
        fi
        systemctl --user daemon-reload
        systemctl --user reset-failed "$SERVICE_NAME" 2>/dev/null || true
        if systemctl --user restart "$SERVICE_NAME" && dumpstar_unit_serves; then
            echo "ERROR: deploy failed; rolled back to ${PREVIOUS_IMAGE_ID:-the previous unit (no previous image recorded)}; ${SERVICE_NAME} is serving." >&2
        else
            dump_unit_diagnostics "$SERVICE_NAME"
            echo "ROLLBACK FAILED: site down (${SERVICE_NAME} does not serve after restoring the previous unit and image)" >&2
        fi
    fi
    exit 1
fi

# The sidecar is a separate unit with its own container: daemon-reload makes
# systemd read the new file, but the running container keeps the arguments it
# was started with until it is restarted. Only when it is already running —
# starting a stopped sidecar is an operator decision (it is gated on backups
# being configured at all), not something a deploy gets to make.
if systemctl --user is-active --quiet "dumpstar-litestream-${INSTANCE}.service"; then
    echo "==> Restarting the Litestream sidecar..."
    systemctl --user restart "dumpstar-litestream-${INSTANCE}.service"
fi

# The outgoing server drained before it stopped (sp-4kt4e8): report the window it refused requests.
DRAIN_LINE="$(unit_log_since "$SERVICE_NAME" "$DRAIN_SINCE" \
    | sed -n 's/^drained in \([0-9.]*\)s (\([0-9]*\) completed, \([0-9]*\) refused.*/refused \3 requests over \1s (\2 in-flight completed)/p' | tail -1)"
echo "==> Drain: ${DRAIN_LINE:-no drain line in the journal}"
echo "==> ${SERVICE_NAME} restarted. Port: podman port systemd-${SERVICE_NAME}"

# /mcp is on only if the unit carried DUMPSTAR_MCP_RESOURCE (sp-tfkp9o): probe the public URL.
if [ "$INSTANCE" = prod ]; then
    echo "==> Checking /mcp is served..."
    bash "$SCRIPT_DIR/mcp-check.sh" || exit 1
fi

# Every timer this checkout ships must be enabled (sp-892w24): enable them, then
# fail the deploy if any still is not.
echo "==> Enabling the shipped timers..."
# shellcheck source=deploy/units-lib.sh
. "$SCRIPT_DIR/units-lib.sh"
dumpstar_units_enable_timers "$INSTANCE"
if dumpstar_units_alerting "$INSTANCE" && ! dumpstar_units_timer_drift "$INSTANCE"; then
    echo "deploy: shipped timers are not all enabled (above)" >&2
    exit 1
fi

# --- Data jobs the change affects (sp-g20aoh) --------------------------------
#
# Last, after the app: a data job that fails must not leave
# the new binary unrestarted. The deployed sha is recorded only when the jobs
# were not needed or succeeded, so a failed run is retried by the next deploy.
# shellcheck source=deploy/data-jobs-lib.sh
. "$SCRIPT_DIR/data-jobs-lib.sh"
if reason="$(dumpstar_data_jobs_needed "$REPO_DIR" "$INSTANCE")"; then
    echo "==> Running the catalog and price jobs: ${reason}"
    dumpstar_data_jobs_run "$INSTANCE"
else
    echo "==> Data jobs not run: ${reason}"
fi
dumpstar_record_deployed_sha "$REPO_DIR" "$INSTANCE"

# --- Valuation (sp-msb7d1) ----------------------------------------------------
# The collection page promises a daily valuation; the timer is what keeps that
# promise, so a deploy arms it, and runs the job once so a tenant is never left
# "not valued yet" until the next 07:30. A failed run pages through the unit's
# OnFailure=; it must not fail a deploy whose app is already up.
if systemctl --user cat "dumpstar-value-snapshots@${INSTANCE}.timer" >/dev/null 2>&1; then
    echo "==> Valuing once now (the timer was enabled with the shipped timers above)..."
    systemctl --user start --wait "dumpstar-value-snapshots@${INSTANCE}.service" \
        || { echo "WARNING: dumpstar-value-snapshots@${INSTANCE}.service failed" >&2; dump_unit_diagnostics "dumpstar-value-snapshots@${INSTANCE}.service"; }
fi
