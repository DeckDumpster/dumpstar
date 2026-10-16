#!/usr/bin/env bash
#
# Set up a Dumpstar container instance (rootless Podman). No sudo.
# Run from within the repo clone.
#
# Usage:
#   bash deploy/setup.sh <instance> [port] [--init] [--test]
#
# Examples:
#   bash deploy/setup.sh prod 8080         # explicit host port
#   bash deploy/setup.sh feature-xyz       # auto-assigned host port
#   bash deploy/setup.sh ci --test         # seed data volume from committed
#                                          #   fixture (no network, ~seconds)
#   bash deploy/setup.sh demo --init       # clone the pre-built seed volume
#
# Container storage: rootless Podman's default store lives under $HOME, which on
# the deployment box is the same disk prod runs from. Set DUMPSTAR_STORE_ROOT=<dir>
# to build this instance's image and volume into an alternate store instead —
# opt-in, so prod (which never sets it) is unaffected. See deploy/store-lib.sh.
# This script scaffolds the host-config file that names that directory
# (~/.config/dumpstar/store.env) but deliberately does not read it: prod is
# installed with this script, and where prod's volumes live is not something a
# host config file gets to change. deploy/ci.sh is what reads it.
#
# Modes:
#   (plain)  Build the image + install the Quadlet. Catalog stays empty —
#            populate it later with deploy/seed.sh.
#   --test   Same, plus seed the new data volume from the committed fixture
#            tests/ui/fixtures/{shared.sqlite,collection.sqlite}. Fully
#            offline — copies the files in via a one-off container.
#   --init   Same, plus clone the reusable `dumpstar-seed-data` volume (built
#            once with `deploy/seed.sh --volume`). Falls back to a full
#            `dumpstar setup` if no seed volume exists.
#
# After plain setup, populate the catalog with deploy/seed.sh and start the
# service with: systemctl --user start dumpstar-<instance>
#
set -euo pipefail

# systemctl --user needs XDG_RUNTIME_DIR; non-interactive shells often lack it.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

# --- Parse arguments --------------------------------------------------------

INIT=false
TEST=false
POSITIONAL=()
for arg in "$@"; do
    case $arg in
        --init) INIT=true ;;
        --test) TEST=true; export DUMPSTAR_UNITS_TEST=1 ;;
        *) POSITIONAL+=("$arg") ;;
    esac
done

if [ ${#POSITIONAL[@]} -lt 1 ]; then
    echo "Usage: bash deploy/setup.sh <instance> [port] [--init] [--test]"
    exit 1
fi

INSTANCE="${POSITIONAL[0]}"
PORT="${POSITIONAL[1]:-0}"
SERVICE_NAME="dumpstar-${INSTANCE}"
VOLUME="dumpstar-${INSTANCE}-data"
IMAGE="localhost/dumpstar:${INSTANCE}"
SEED_VOLUME="dumpstar-seed-data"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

if ! command -v podman >/dev/null 2>&1; then
    echo "ERROR: podman not found. Install it: sudo apt install podman"
    exit 1
fi

# Opt-in alternate container store (pd-fite). No-op unless DUMPSTAR_STORE_ROOT is
# set — and prod never sets it.
#
# An instance that already exists keeps the store it already lives in, for the
# same reason it keeps the port it already publishes (below): re-running setup.sh
# is how unit-file changes reach it, and that must not silently move its image
# and volume into whatever store the calling shell had activated, leaving the
# real ones behind with the unit no longer pointing at them (pd-9rxf). A store
# passed explicitly still wins, so moving an instance on purpose still works.
# shellcheck source=deploy/store-lib.sh
. "$SCRIPT_DIR/store-lib.sh"
dumpstar_store_adopt_instance "$INSTANCE"
dumpstar_store_activate

if ! grep -q "Linger=yes" <<<"$(loginctl show-user "${USER:-$(id -un)}" -p Linger 2>/dev/null)"; then
    echo "WARNING: linger not enabled — services stop when you log out."
    echo "  Fix: loginctl enable-linger ${USER:-$(id -un)}"
fi

# --- Build the image --------------------------------------------------------
# Or tag one already built: deploy/ci.sh calls this script twice per run and
# three other gates build the same content again, so it builds once up front and
# exports DUMPSTAR_PREBUILT_IMAGE. Unset — which is prod, always — this is the
# same `podman build` it has always been. See deploy/image-lib.sh.

# shellcheck source=deploy/image-lib.sh
. "$SCRIPT_DIR/image-lib.sh"
if [ -n "${DUMPSTAR_PREBUILT_IMAGE:-}" ]; then
    echo "==> Tagging ${DUMPSTAR_PREBUILT_IMAGE} as dumpstar:${INSTANCE} (built once this run)..."
else
    echo "==> Building image dumpstar:${INSTANCE}..."
fi
dumpstar_image_ensure "dumpstar:${INSTANCE}" "$REPO_DIR"
podman tag "dumpstar:${INSTANCE}" dumpstar:latest

# --- Install the unit files -------------------------------------------------
# Every Quadlet + systemd-user unit this checkout ships for the instance, in one
# place, because deploy.sh installs exactly the same set (pd-2t6u). An existing
# instance keeps the port it already publishes unless one is passed explicitly.

echo "==> Installing unit files..."
# shellcheck source=deploy/units-lib.sh
. "$SCRIPT_DIR/units-lib.sh"
dumpstar_units_install "$INSTANCE" "$PORT"
dumpstar_units_report
# shellcheck source=deploy/config-lib.sh
. "$SCRIPT_DIR/config-lib.sh"

# Every per-instance and host-wide config file the units read, from the one scaffold
# deploy.sh uses too (sp-kaj3xv): setup.sh and deploy.sh cannot disagree on what an
# instance needs. LS_CONF_DIR stays set for the key notes below.
LS_CONF_DIR="${HOME}/.config/dumpstar/${INSTANCE}"
dumpstar_scaffold_config "$INSTANCE"

# A --test instance is verified against a throwaway JWKS, never Cloudflare: the
# server checks a JWT on every /api request and has no bypass, so the instance
# is started WITH Access configured and the UI tests present a token signed by
# the key in ${LS_CONF_DIR}/test-jwks (UI_TEST_JWKS_DIR). The JWKS server is a
# transient user unit so it outlives this script, and is stopped by
# `systemctl --user stop dumpstar-test-jwks-${INSTANCE}`.
if [ "$TEST" = true ]; then
    # shellcheck source=tests/lib/access.sh
    . "$REPO_DIR/tests/lib/access.sh"
    TEST_JWKS_UNIT="dumpstar-test-jwks-${INSTANCE}"
    TEST_JWKS_DIR="${LS_CONF_DIR}/test-jwks"
    systemctl --user stop "$TEST_JWKS_UNIT" 2>/dev/null || true
    rm -rf "$TEST_JWKS_DIR"
    mkdir -p "$TEST_JWKS_DIR"
    systemd-run --user --quiet --unit "$TEST_JWKS_UNIT" \
        bash "$REPO_DIR/tests/lib/test-jwks.sh" serve "$TEST_JWKS_DIR"
    for _ in $(seq 1 100); do
        [ -f "$TEST_JWKS_DIR/jwks.json" ] && break
        sleep 0.1
    done
    if [ ! -f "$TEST_JWKS_DIR/jwks.json" ]; then
        echo "ERROR: the test JWKS server wrote no jwks.json (cargo must build dumpstar-test-jwks)."
        exit 1
    fi
    access_env_file "$TEST_JWKS_DIR" "${LS_CONF_DIR}/access.env"
    echo "    Test Access config written to ${LS_CONF_DIR}/access.env (JWKS unit ${TEST_JWKS_UNIT})."
fi

systemctl --user daemon-reload

# --- Optional data volume seeding ------------------------------------------

seed_from_fixture() {
    # Copy the committed fixture sqlite files onto the instance's data volume
    # via a one-off container. No network — purely a local file copy.
    local fixture_dir="$REPO_DIR/tests/ui/fixtures"
    local shared="${fixture_dir}/shared.sqlite"
    local collection="${fixture_dir}/collection.sqlite"

    # Built from text sources, never committed (cached by input hash).
    bash "${fixture_dir}/build.sh" >/dev/null

    if [ ! -f "$shared" ] || [ ! -f "$collection" ]; then
        echo "ERROR: fixture files not found under ${fixture_dir}"
        echo "    Expected shared.sqlite and collection.sqlite."
        exit 1
    fi

    echo "==> Seeding ${VOLUME} from committed fixture (offline)..."
    podman volume exists "$VOLUME" 2>/dev/null || podman volume create "$VOLUME" >/dev/null

    local temp="dumpstar-fixture-$$"
    podman run -d --name "$temp" \
        -v "${VOLUME}:/data:Z" \
        --entrypoint sleep \
        "$IMAGE" infinity >/dev/null
    # shellcheck disable=SC2064
    trap "podman rm -f '$temp' >/dev/null 2>&1 || true" RETURN

    # Tenant collection DBs live under /data/tenants (deploy/TENANTS.md); the
    # catalog stays at the root of the data dir, shared by every tenant.
    podman exec "$temp" mkdir -p /data/tenants
    podman cp "$shared"     "${temp}:/data/shared.sqlite"
    podman cp "$collection" "${temp}:/data/tenants/collection.sqlite"
    podman rm -f "$temp" >/dev/null
    trap - RETURN
    echo "    Fixture data installed."
}

seed_from_volume() {
    # Clone the reusable dumpstar-seed-data volume, or fall back to a full
    # `dumpstar setup` if it has not been built yet.
    if podman volume exists "$SEED_VOLUME" 2>/dev/null; then
        echo "==> Cloning seed volume into ${VOLUME}..."
        podman volume exists "$VOLUME" 2>/dev/null || podman volume create "$VOLUME" >/dev/null
        podman volume export "$SEED_VOLUME" | podman volume import "$VOLUME" -
        echo "    Done (cloned from ${SEED_VOLUME})."
    else
        echo "==> No seed volume found — running full 'dumpstar setup' (slow)..."
        echo "    TIP: build a reusable seed volume once: bash deploy/seed.sh --volume"
        bash "$SCRIPT_DIR/seed.sh" "$INSTANCE"
    fi
}

if [ "$TEST" = true ]; then
    seed_from_fixture
elif [ "$INIT" = true ]; then
    seed_from_volume
fi

# --- Done -------------------------------------------------------------------

echo ""
echo "==> Setup complete."
if [ "$TEST" = true ] || [ "$INIT" = true ]; then
    echo "    Data volume seeded."
else
    echo "    Populate catalog:  bash deploy/seed.sh ${INSTANCE}"
fi
echo "    Start:             systemctl --user start ${SERVICE_NAME}"
echo "    Port:              podman port systemd-${SERVICE_NAME}"
echo "    Logs:              journalctl --user -u ${SERVICE_NAME} -f"
echo "    Nightly catalog:   the refresh LANDS and the derive BUILDS — enable BOTH or neither."
echo "                       The refresh refuses to run while the derive timer is off (pd-lunn),"
echo "                       because landing without deriving freezes the catalog silently:"
echo "                       systemctl --user enable --now dumpstar-derive@${INSTANCE}.timer"
echo "                       systemctl --user enable --now dumpstar-refresh@${INSTANCE}.timer"
echo "    Uptime (L0):       edit ~/.config/dumpstar/${INSTANCE}/alerts.env (DUMPSTAR_UPTIME_PING_URL, a"
echo "                       SEPARATE check from the backup one; the probe itself hits the origin on loopback), then:"
echo "                       systemctl --user enable --now dumpstar-heartbeat@${INSTANCE}.timer"
echo "    Backup-check (L1): edit ~/.config/dumpstar/${INSTANCE}/alerts.env (ping URL), then:"
echo "                       systemctl --user enable --now dumpstar-backup-check@${INSTANCE}.timer"
echo "    Disk alert (L4):   edit ~/.config/dumpstar/alerts.env (Pushover), then:"
echo "                       systemctl --user enable --now dumpstar-diskcheck.timer"
echo "    Scratchpad reaper: abandoned Claude session scratchpads under \$TMPDIR — the disk"
echo "                       (L4b)  ci.sh measures and the one that blocked it at 99%. Dry-run it"
echo "                       first; it prints every directory it would remove:"
echo "                       bash ${REPO_DIR}/deploy/tmpreap.sh --dry-run"
echo "                       systemctl --user enable --now dumpstar-tmpreap.timer"
echo "    Restore (DR):      deploy/RESTORE.md  (off-box S3 via deploy/restore-litestream.sh)"
echo "    Litestream backup: edit ~/.config/dumpstar/${INSTANCE}/{litestream.env,aws/}, then: systemctl --user start dumpstar-litestream-${INSTANCE}.service"
echo "    Teardown:          bash deploy/teardown.sh ${INSTANCE}"
