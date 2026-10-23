#!/usr/bin/env bash
# Scaffold per-instance config files the Quadlet units require.
# Every function here is idempotent: it writes only when the file is absent,
# so it is safe to call on every setup AND every deploy (db-g6ku).

# Write the Cloudflare Access config file for <instance> into <conf_dir> if it
# does not already exist. Access is mandatory: the server refuses to start until
# the three values are filled in, and there is no mode that skips it.
# The Quadlet unit loads this with 'EnvironmentFile=...' (no leading '-', which
# Quadlet does not support), so the file must always exist before the unit
# starts.
#
# The scaffolded env files are the templates under deploy/env/, rendered with the same
# parameters the Quadlets get (deploy/render-lib.sh): one source for what an instance reads.
# dumpstar_render_env <access|oidc|metrics|litestream> <instance>  (to stdout)
dumpstar_render_env() {
    local dir hostname
    dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    # shellcheck source=deploy/render-lib.sh
    . "$dir/render-lib.sh"
    hostname="$DUMPSTAR_PROD_HOSTNAME"
    local args=()
    mapfile -t args < <(dumpstar_render_sed_args "$2" "$hostname" "" "dumpstar-$2-data" "%h/.config/dumpstar/$2" "$2")
    sed "${args[@]}" "$dir/env/$1.env"
}

# Usage: dumpstar_scaffold_access_env <instance> <conf_dir>
dumpstar_scaffold_access_env() {
    local instance="$1" conf_dir="$2"
    if [ ! -f "${conf_dir}/access.env" ]; then
        mkdir -p "$conf_dir"
        dumpstar_render_env access "$instance" > "${conf_dir}/access.env"
        chmod 600 "${conf_dir}/access.env"
        echo "    Wrote ${conf_dir}/access.env — fill the CHANGE_ME values; dumpstar will not start without them."
        echo "    See deploy/TENANTS.md §\"Configuring Access for a deployment\" for instructions."
    fi
}

# Write the Pocket ID sign-in config for <instance> into <conf_dir> if it does not already
# exist, commented out and mode 600. The Quadlet unit loads it with a mandatory
# 'EnvironmentFile=', so the file always exists. Left commented, the server runs behind
# Cloudflare Access alone; once ANY DUMPSTAR_OIDC_* line is set it signs users in itself
# (dumpstar serve) and all of them are required. The client id and secret are placed here
# by the operator on the host — never in git or chat.
#
# Usage: dumpstar_scaffold_oidc_env <instance> <conf_dir>
dumpstar_scaffold_oidc_env() {
    local instance="$1" conf_dir="$2"
    if [ ! -f "${conf_dir}/oidc.env" ]; then
        mkdir -p "$conf_dir"
        dumpstar_render_env oidc "$instance" > "${conf_dir}/oidc.env"
        chmod 600 "${conf_dir}/oidc.env"
        echo "    Wrote ${conf_dir}/oidc.env — commented out; fill it to sign in with Pocket ID."
    fi
}

# The product metrics stack's OTLP endpoint (dumpster-kit deploy/metrics: VictoriaMetrics on the
# the host's tailnet address, port 8428 — not its loopback, which VictoriaMetrics does not listen
# on, and not 10.0.2.2, which is that loopback; sp-hxsauf).
DUMPSTAR_PRODUCT_OTLP_ENDPOINT="http://100.95.160.116:8428/opentelemetry/v1/metrics"

# Write the OTLP metrics config for <instance> into <conf_dir>, mode 600. The Quadlet unit loads
# it with a mandatory 'EnvironmentFile=' (Quadlet rejects a leading '-'), so the file always
# exists. The prod instance gets the product stack's endpoint set, with no hand edit; a prod
# file still holding the old CHANGE_ME placeholder is rewritten. Any other instance (test
# instances) stays commented out, so it never feeds prod's dashboard.
#
# Usage: dumpstar_scaffold_metrics_env <instance> <conf_dir>
dumpstar_scaffold_metrics_env() {
    local instance="$1" conf_dir="$2" f="$2/metrics.env"
    if [ "$instance" = prod ]; then
        if [ -f "$f" ] && grep -q '^DUMPSTER_OTLP_METRICS_ENDPOINT=' "$f"; then
            return 0
        fi
        mkdir -p "$conf_dir"
        dumpstar_render_env metrics "$instance" > "$f"
        chmod 600 "$f"
        echo "    Wrote ${f} — metrics go to ${DUMPSTAR_PRODUCT_OTLP_ENDPOINT}."
    elif [ ! -f "$f" ]; then
        mkdir -p "$conf_dir"
        cat > "$f" <<EOF
# OTLP metrics push for instance '${instance}': off. Only prod sends to the product stack;
# set DUMPSTER_OTLP_METRICS_ENDPOINT here to push from this instance.
#DUMPSTER_OTLP_METRICS_ENDPOINT=${DUMPSTAR_PRODUCT_OTLP_ENDPOINT}
EOF
        chmod 600 "$f"
        echo "    Wrote ${f} — commented out; this instance sends no metrics."
    fi
}

# Scaffold every config file the instance's units read: the host-wide alerts.env,
# store.env and lake.env, and the per-instance litestream.env, alerts.env and
# access.env. Idempotent and never clobbering; setup.sh and deploy.sh both call it
# (sp-kaj3xv), so a file a unit needs exists after either. The body is deliberately
# unindented: it is mostly heredocs, whose content is the file.
#
# Usage: dumpstar_scaffold_config <instance>
dumpstar_scaffold_config() {
local INSTANCE="$1"
local LS_CONF_DIR ALERTS_ENV STORE_ENV LAKE_ENV ADDED KEY_MODE
# Scaffold the host-wide alert config (Pushover creds + disk threshold). Secrets
# NEVER live in the repo; this only writes a template and never clobbers it.
ALERTS_ENV="${HOME}/.config/dumpstar/alerts.env"
if [ ! -f "$ALERTS_ENV" ]; then
    mkdir -p "${HOME}/.config/dumpstar"
    cat > "$ALERTS_ENV" <<'EOF'
# Host-wide alert config for Dumpstar (dumpstar-ivq). Fill CHANGE_ME.
# Pushover push channel (Layers 2 + 4, and Layer 1's fast detail push).
# Create an app at https://pushover.net/apps/build for the token.
PUSHOVER_TOKEN=CHANGE_ME
PUSHOVER_USER=CHANGE_ME
# Low-disk alert (Layer 4): filesystem to watch, and the two arms that page.
# The free-space arm is what buys lead time — deploy/ci.sh refuses to build under
# DUMPSTAR_DISK_FLOOR_GB (10) free, and a percentage cannot say how close that is.
# Blank DUMPSTAR_DISK_WARN_GB means twice the floor; it MUST clear the floor, and
# a value that does not is refused rather than clamped (pd-smcp).
DUMPSTAR_DISK_PATH=
DUMPSTAR_DISK_WARN_GB=
DUMPSTAR_DISK_THRESHOLD=90
EOF
    chmod 600 "$ALERTS_ENV"
    echo "    Wrote ${ALERTS_ENV} (fill PUSHOVER_TOKEN/USER)."
fi

# Scaffold the host-wide container-store config (pd-rf7c). Which disk non-prod
# container storage belongs on is a fact about THIS box, so it is host config
# rather than a repo constant — and it is scaffolded commented-out so a new box
# has the knob visible instead of undiscoverable. Written but never read by this
# script: setup.sh honours DUMPSTAR_STORE_ROOT from its environment only, so a
# store.env that opts in cannot silently relocate a prod deploy.
STORE_ENV="${HOME}/.config/dumpstar/store.env"
if [ ! -f "$STORE_ENV" ]; then
    mkdir -p "${HOME}/.config/dumpstar"
    cat > "$STORE_ENV" <<'EOF'
# Host-wide container-store config for Dumpstar (pd-rf7c).
#
# Rootless Podman keeps images, layers and volumes under $HOME. Where $HOME
# shares a disk with something that must not run out of space — on this
# project's deployment box, prod itself — name a directory on another
# filesystem here and non-prod container storage goes there instead.
#
# Read by deploy/ci.sh. An explicit DUMPSTAR_STORE_ROOT in the environment wins
# over this file, including an explicit empty one (that is how a single run opts
# back out). Left commented out, everything uses Podman's default store — which
# is what prod uses, always, on every box.
#
# To delete the store named here, follow the recipe in deploy/store-lib.sh's
# header. NEVER `podman system reset`: it is not scoped by --root/--runroot, and
# aimed at a throwaway store it took prod down (pd-rkrf).
#DUMPSTAR_STORE_ROOT=/big/disk/dumpstar-nonprod-store
EOF
    echo "    Wrote ${STORE_ENV} (container store; commented out = Podman's default)."
fi

# Scaffold the host-wide raw-landing-zone config (pd-th42). The lake bucket is a
# fact about this AWS account, not a repo constant, so it is host config in the
# same shape as alerts.env and store.env — and it is scaffolded commented-out so
# a new box has the knob visible rather than undiscoverable. Nothing lands until
# this file names a bucket, and an unconfigured lake makes the commands that
# need one REFUSE rather than land nothing. Since pd-lunn `dumpstar data refresh`
# is one of them unconditionally — it lands and builds nothing, so a run with
# nowhere to land has nothing to do. See deploy/LAKE.md.
LAKE_ENV="${HOME}/.config/dumpstar/lake.env"
if [ ! -f "$LAKE_ENV" ]; then
    mkdir -p "${HOME}/.config/dumpstar"
    cat > "$LAKE_ENV" <<'EOF'
# Host-wide raw-landing-zone config for Dumpstar (pd-th42).
#
# `dumpstar data refresh` writes every upstream response it fetches to this
# bucket, immutably, before parsing it:
#
#   raw/source=<tcgcsv|pokemontcgio|pokemon-tcg-data>/dataset=<...>/
#       ingest_date=YYYY-MM-DD/run=<ULID>/part-NNNN.<ext>.zst  +  _manifest.json
#
# This MUST be a DIFFERENT bucket from LITESTREAM_S3_BUCKET. The backup bucket
# holds the only irreplaceable data in the system — tenant databases and the
# registry — while everything in the lake is reproducible by construction.
# Keeping them apart is what stops a lifecycle rule written for the lake from
# ever reaching the backups. Same account, same AWS_PROFILE=dumpstar role path,
# so there is no new credential story: the SDK assumes the role and refreshes.
#
# There is deliberately NO lifecycle rule on raw/. See deploy/LAKE.md.
#DUMPSTAR_LAKE_S3_BUCKET=CHANGE_ME
#DUMPSTAR_LAKE_S3_REGION=us-west-2
# Optional key prefix inside the bucket; unset means raw/ sits at the root.
#DUMPSTAR_LAKE_S3_PREFIX=
# Optional endpoint override — how a MinIO stands in for S3 in a test.
#DUMPSTAR_LAKE_S3_ENDPOINT=
EOF
    chmod 600 "$LAKE_ENV"
    echo "    Wrote ${LAKE_ENV} (raw landing zone; commented out = landing off)."
fi

# --- Litestream backup sidecar config (dumpstar-8ch.3) ------------------
# The sidecar's Quadlet unit is installed above with the rest of them; what is
# left is its per-instance config (S3 target + AWS creds). Secrets NEVER live in
# the repo; this only writes a template and never clobbers existing config.
LS_CONF_DIR="${HOME}/.config/dumpstar/${INSTANCE}"
mkdir -p "${LS_CONF_DIR}/aws"
chmod 700 "${LS_CONF_DIR}" "${LS_CONF_DIR}/aws"
if [ ! -f "${LS_CONF_DIR}/litestream.env" ]; then
    dumpstar_render_env litestream "$INSTANCE" > "${LS_CONF_DIR}/litestream.env"
    chmod 600 "${LS_CONF_DIR}/litestream.env"
    # NOTE: aws/config + the bootstrap key are NOT created here. aws/config gates
    # the sidecar auto-start (so it won't crash-loop unconfigured), and the key is
    # the only secret — it lives in a podman secret, never a plaintext dotfile.
    echo "    Wrote ${LS_CONF_DIR}/litestream.env (fill CHANGE_ME)."
    echo "    To ENABLE backups (assume-role; the bootstrap key never touches the repo):"
    echo "      1. Fill litestream.env (bucket/path)."
    echo "      2. Create ${LS_CONF_DIR}/aws/config (role profile — gates auto-start):"
    echo "           [profile dumpstar]"
    echo "           role_arn = arn:aws:iam::ACCOUNT:role/your-backup-role"
    echo "           source_profile = bootstrap"
    echo "           region = us-west-2"
    echo "      3. Store the bootstrap key in a podman secret (NOT a file):"
    echo "           printf '[bootstrap]\\naws_access_key_id=K\\naws_secret_access_key=S\\n' | podman secret create dumpstar-${INSTANCE}-backup-s3 -"
    echo "    Sidecar auto-starts once aws/config exists + the secret is present."
fi

# Backfill keys an OLDER litestream.env predates (pd-nd6w: the registry entry).
# The block above deliberately never clobbers an operator's config, which is
# also why it cannot introduce a new key into one — and deploy/litestream.yml
# now references two. An unset db path is fatal to the sidecar at startup
# (measured: "must specify either 'path' or 'dir'"), so an un-backfilled
# instance does not silently drop the registry from the replicated set; it
# refuses to run. Re-running this script is the fix, so make it BE the fix.
#
# Append-only, and only for keys that are absent: nothing here rewrites a value
# the operator chose. The defaults match the template above.
if [ -f "${LS_CONF_DIR}/litestream.env" ]; then
    ADDED=()
    grep -q '^LITESTREAM_REGISTRY_DB=' "${LS_CONF_DIR}/litestream.env" || ADDED+=("LITESTREAM_REGISTRY_DB=/data/registry.sqlite")
    grep -q '^LITESTREAM_S3_REGISTRY_PATH=' "${LS_CONF_DIR}/litestream.env" || ADDED+=("LITESTREAM_S3_REGISTRY_PATH=${INSTANCE}/registry.sqlite")
    if [ ${#ADDED[@]} -gt 0 ]; then
        {
            echo ""
            echo "# Added by deploy/setup.sh (pd-nd6w): the user registry joins the replicated"
            echo "# set. It is replicated as itself, beside LITESTREAM_S3_PATH — not inside it."
            printf '%s\n' "${ADDED[@]}"
        } >> "${LS_CONF_DIR}/litestream.env"
        echo "==> Backfilled ${#ADDED[@]} registry key(s) into ${LS_CONF_DIR}/litestream.env:"
        printf '      %s\n' "${ADDED[@]}"
        echo "    Restart the sidecar to pick them up:"
        echo "      systemctl --user restart dumpstar-litestream-${INSTANCE}.service"
    fi
fi

# --- Tenant-zone master key (pd-ulds) --------------------------------------
# NOT created here, on purpose, and for the same reason the bootstrap key is
# not: a secret minted by a script nobody was watching is a secret nobody
# backed up — and this one has no second copy anywhere by design. So setup.sh
# prepares the DIRECTORY (already done above, 700, beside litestream.env) and
# names the step; `dumpstar keys init` is the operator's deliberate act.
#
# There is exactly one master key and every tenant's key is derived from it, so
# overwriting it destroys every tenant's key at once. `keys init` refuses to
# overwrite and there is no --force.
if [ ! -f "${LS_CONF_DIR}/tenant-master.key" ]; then
    echo "    No tenant-zone master key for '${INSTANCE}' (${LS_CONF_DIR}/tenant-master.key)."
    echo "    Crypto-shredding is off until there is one. To create it:"
    echo "      1. bash deploy/keys.sh ${INSTANCE} init"
    echo "      2. bash deploy/keys.sh ${INSTANCE} backup --yes   # into your password manager,"
    echo "         the same place the [bootstrap] key above lives. There is no other copy."
    echo "    Losing that key is NOT the same event as deleting a tenant — see deploy/KEYS.md."
else
    KEY_MODE="$(stat -c '%a' "${LS_CONF_DIR}/tenant-master.key")"
    if [ "$KEY_MODE" != "600" ]; then
        echo "    WARNING: ${LS_CONF_DIR}/tenant-master.key is mode ${KEY_MODE}, not 600." >&2
        echo "             Fix it: chmod 600 ${LS_CONF_DIR}/tenant-master.key" >&2
    fi
fi

# Per-instance alert config: the healthchecks.io ping URL for THIS instance's
# backup-freshness dead-man's switch (dumpstar-ivq.2). Empty = Layer 1 off.
if [ ! -f "${LS_CONF_DIR}/alerts.env" ]; then
    cat > "${LS_CONF_DIR}/alerts.env" <<EOF
# Per-instance backup-alarming config for '${INSTANCE}'. Empty PING_URL = Layer 1
# disabled. Create a check at https://healthchecks.io (period ~6h, grace ~3h),
# wire its Pushover integration, and paste its ping URL here.
DUMPSTAR_BACKUP_PING_URL=
# Alert when the newest S3 snapshot is older than this (hours; daily snapshots).
DUMPSTAR_BACKUP_MAX_AGE_HOURS=36
EOF
    chmod 600 "${LS_CONF_DIR}/alerts.env"
    echo "    Wrote ${LS_CONF_DIR}/alerts.env (paste the healthchecks.io ping URL to arm Layer 1)."
fi

# Per-instance Cloudflare Access + multi-tenant config (db-g6ku: extracted into
# config-lib.sh so deploy.sh can scaffold it for existing instances too).
dumpstar_scaffold_access_env "$INSTANCE" "$LS_CONF_DIR"
dumpstar_scaffold_oidc_env "$INSTANCE" "$LS_CONF_DIR"
dumpstar_scaffold_metrics_env "$INSTANCE" "$LS_CONF_DIR"
}

# --- Required keys, checked before a deploy restarts anything (sp-kaj3xv) ----
#
# A release that makes a new config key mandatory (the way Access did) would take
# the instance down if it were restarted into an env file that predates the key:
# the server refuses to start, the unit crash-loops, the site is gone. So the
# keys the shipped server requires are listed HERE, one line each, and the
# deploy script checks them before it builds, installs a unit or restarts. A key
# with a default is appended to the file; a key without one makes the deploy
# stop with the old container still serving.
#
# Adding a required key to the server means adding a line below. Format:
#   <file under the instance conf dir>|<KEY>|<default, empty = operator must supply>
DUMPSTAR_REQUIRED_CONFIG=(
    "access.env|DUMPSTAR_ACCESS_TEAM_DOMAIN|"
    "access.env|DUMPSTAR_ACCESS_AUD|"
    "access.env|DUMPSTAR_ACCESS_JWKS_URL|"
)

# Value lines may not carry a trailing "# comment": systemd EnvironmentFile and
# podman --env-file keep it as part of the value. Prints each offending line.
dumpstar_config_inline_comments() {
    local f
    for f in "$@"; do
        [ -f "$f" ] || continue
        grep -nE '^[A-Za-z_][A-Za-z0-9_]*=.*[[:space:]]#' "$f" | sed "s|^|${f}:|" || true
    done
}

# Backfill defaulted required keys and report the rest. Returns 1, naming every
# missing key and every inline comment, when the instance could not start.
#
# Usage: dumpstar_config_converge <conf_dir>
dumpstar_config_converge() {
    local conf_dir="$1" entry file key default val bad=0 out
    for entry in "${DUMPSTAR_REQUIRED_CONFIG[@]}"; do
        IFS='|' read -r file key default <<<"$entry"
        val="$(sed -n "s/^${key}=//p" "${conf_dir}/${file}" 2>/dev/null | tail -1)"
        if [ -z "$val" ] || [ "$val" = CHANGE_ME ]; then
            if [ -n "$default" ] && [ -f "${conf_dir}/${file}" ]; then
                printf '%s=%s\n' "$key" "$default" >> "${conf_dir}/${file}"
                echo "    Backfilled ${key} into ${conf_dir}/${file}"
            else
                echo "    MISSING required config: ${key} in ${conf_dir}/${file}" >&2
                bad=1
            fi
        fi
    done
    out="$(dumpstar_config_inline_comments "${conf_dir}"/*.env)"
    if [ -n "$out" ]; then
        echo "    Inline # comment on a value line (the comment becomes part of the value):" >&2
        echo "$out" | sed 's/^/      /' >&2
        bad=1
    fi
    return $bad
}
