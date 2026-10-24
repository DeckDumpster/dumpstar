#!/usr/bin/env bash
# Run the data jobs a deploy's change affects (sp-g20aoh).
#
# The catalog and the prices are written by timers (refresh 06:00 lands the
# upstreams in the lake, derive 07:00 builds shared.sqlite from them, catalog-sync
# 08:00 fetches the pictures, prices 07:00), so a deployed fix to the crosswalk or the price derivation
# showed no effect for up to a day: sp-9uiu2z deployed at 09:40Z and CST/10002
# was still unpriced at 17:30Z. A deploy that changes what those jobs compute
# runs them itself, once, after the service restarts.
#
# Sourced by deploy.sh; deploy/data-jobs.sh is the on-demand entry point.

# Paths whose change alters what catalog-apply or the price build writes.
DUMPSTAR_DATA_JOBS_PATHS='^(crates/dumpstar-catalog/|crates/dumpstar-ingest/|crates/dumpstar-derive/|crates/dumpstar-lakehouse/|data/remedial/|lake/|deploy/prices\.sh$|deploy/dumpstar-(catalog-sync|prices)\.service$)'

dumpstar_deployed_sha_file() {
    echo "${XDG_STATE_HOME:-$HOME/.local/state}/dumpstar/${1}.deployed-sha"
}

# dumpstar_data_jobs_needed <repo> <instance> -> 0 when the jobs should run.
# Prints the reason on stdout either way.
dumpstar_data_jobs_needed() {
    local repo="$1" instance="$2" file old new hits
    file="$(dumpstar_deployed_sha_file "$instance")"
    new="$(git -C "$repo" rev-parse HEAD 2>/dev/null)" || { echo "no git HEAD to compare; running"; return 0; }
    old="$(cat "$file" 2>/dev/null || true)"
    if [ -z "$old" ]; then echo "no previously deployed sha recorded; running"; return 0; fi
    if [ "$old" = "$new" ]; then echo "${new:0:12} was already deployed; nothing changed"; return 1; fi
    if ! hits="$(git -C "$repo" diff --name-only "$old" "$new" 2>/dev/null)"; then
        echo "previous sha ${old:0:12} unknown to this checkout; running"; return 0
    fi
    hits="$(grep -E "$DUMPSTAR_DATA_JOBS_PATHS" <<<"$hits" || true)"
    if [ -n "$hits" ]; then
        echo "catalog/price code changed since ${old:0:12}: $(head -3 <<<"$hits" | paste -sd, -)"
        return 0
    fi
    echo "no catalog/price change since ${old:0:12}; skipped"
    return 1
}

dumpstar_record_deployed_sha() {
    local repo="$1" instance="$2" file
    file="$(dumpstar_deployed_sha_file "$instance")"
    mkdir -p "$(dirname "$file")"
    git -C "$repo" rev-parse HEAD >"$file" 2>/dev/null || true
}

# dumpstar_data_jobs_run <instance>: land the upstreams, derive the catalog from
# what landed, fetch the pictures, then prices (sp-6i11rn: the catalog is derived
# from the lake's raw, so a change to what it derives is proven by landing and
# deriving, not by an online apply). Each is its systemd unit, so the run is
# journaled exactly as the timer's would be. A box with no lake configured skips
# the first two by their own ConditionPathExists.
# dumpstar_lake_installed lives in lake-lib.sh; deploy.sh sources it, data-jobs.sh may not.
if ! declare -F dumpstar_lake_installed >/dev/null; then
    # shellcheck source=deploy/lake-lib.sh
    . "$(dirname "${BASH_SOURCE[0]}")/lake-lib.sh"
fi
dumpstar_data_jobs_run() {
    local instance="$1" unit
    for unit in "dumpstar-refresh@${instance}.service" "dumpstar-derive@${instance}.service" "dumpstar-catalog-sync@${instance}.service" "dumpstar-prices@${instance}.service"; do
        # The prices unit is a lake job; an instance with no lakehouse has nothing
        # for it to run (deploy.sh skips the lake image for the same reason).
        if [ "$unit" = "dumpstar-prices@${instance}.service" ] && ! dumpstar_lake_installed "$instance"; then
            echo "==> data job ${unit}: no lakehouse installed for '${instance}'; skipped"
            continue
        fi
        if ! systemctl --user cat "$unit" >/dev/null 2>&1; then
            echo "==> data job ${unit}: not installed on this instance; skipped"
            continue
        fi
        echo "==> data job ${unit}: running"
        if systemctl --user start --wait "$unit"; then
            echo "==> data job ${unit}: done"
        else
            echo "ERROR: data job ${unit} failed; see journalctl --user -u ${unit}" >&2
            return 1
        fi
    done
}
