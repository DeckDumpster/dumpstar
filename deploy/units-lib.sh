#!/usr/bin/env bash
#
# Installing this checkout's systemd/Quadlet unit files for an instance (pd-2t6u).
#
# The unit files under deploy/ are TEMPLATES: {{INSTANCE}}, {{PORT}} and
# {{REPO_DIR}} are sed-substituted into $HOME/.config/{containers/systemd,
# systemd/user} at install time. So "the unit prod runs" is a copy, and a copy
# only tracks the repo if something rewrites it.
#
# Nothing did. deploy/setup.sh wrote the units; deploy/deploy.sh — the command
# an operator actually runs to ship a change — rebuilt the image and restarted,
# and reinstalled the units ONLY when the instance did not exist yet. Every
# unit-file change since an instance was first set up therefore stayed in the
# repo. Observed on the prod box 2026-08-09: dumpstar-litestream-prod.container
# was still the pre-multi-tenant template, missing the Jun 2026
# `OnFailure=dumpstar-alert@%n.service` — so the sidecar that silently stopped
# replicating had no failure alerting wired, while the repo said it did.
#
# Hence one function, used by both scripts, so "install the units" cannot mean
# two different sets of files. It is a CONVERGE step: every unit is re-rendered
# from the template every time, and the ones whose content actually changed are
# named on stdout. A deploy that silently rewrites nothing and a deploy that
# silently rewrites everything look the same otherwise, and this project has
# already paid for one of those.
#
# What is NOT here, deliberately: the config scaffolding (alerts.env, store.env,
# litestream.env). Those are an operator's files with an operator's secrets in
# them, created once and never clobbered — creating them is setting an instance
# UP, not shipping code to it. setup.sh keeps that job.
#
# The caller must have sourced deploy/store-lib.sh and activated the store it
# wants stamped into the generated Quadlet units — the flags are read from the
# environment dumpstar_store_activate exports, not re-derived here.
#
# Sourced, not executed.

# Unit basenames written by the last dumpstar_units_install call whose content
# changed. Empty means the installed units already matched this checkout.
DUMPSTAR_UNITS_CHANGED=()

# The loopback port prod publishes the server on. Recorded in deploy/README.md; the
# Cloudflare tunnel route (a dashboard step) targets it. The number lives in
# deploy/prod-origin-port, configuration rather than a literal in a script.
DUMPSTAR_PROD_PORT="$(tr -d '[:space:]' <"$(dirname "${BASH_SOURCE[0]}")/prod-origin-port")"

# The origin health URL an installed instance serves on, read from its Quadlet unit;
# fails if there is no unit or it does not publish on loopback.
#
# Usage: dumpstar_origin_url <instance>
dumpstar_origin_url() {
    local f="${HOME}/.config/containers/systemd/dumpstar-${1}.container" port
    [ -f "$f" ] || return 1
    port="$(sed -n 's|^PublishPort=127\.0\.0\.1:\([0-9]\{1,\}\):8080$|\1|p' "$f" | head -1)"
    [ -n "$port" ] || return 1
    printf 'http://127.0.0.1:%s/health\n' "$port"
}

# The alerting predicate + the OnFailure gate live in alert-gate.sh so the alert
# unit and the renderer cannot disagree about which instances may page (pd-n0lf).
# shellcheck source=deploy/alert-gate.sh
. "$(dirname "${BASH_SOURCE[0]}")/alert-gate.sh"

# ── HOST-WIDE UNITS: ONE COPY PER BOX, AND IT IS NOT YOURS (pd-onyd) ─────────
#
# Everything this file writes into ~/.config/systemd/user is a SINGLE FILE
# SHARED BY EVERY INSTANCE. The %i templates look per-instance and are not:
# `dumpstar-refresh@.service` is one file backing dumpstar-refresh@prod,
# dumpstar-refresh@ci-9f2c1a and every other instance at once, and
# dumpstar-diskcheck is not even templated. Each of them bakes {{REPO_DIR}} into
# an ExecStart. (The two Quadlet units ARE per-instance —
# dumpstar-<instance>.container and dumpstar-litestream-<instance>.container — so
# they are written unconditionally and are not in the list below.)
#
# So "install the units" used to mean "point every instance's alerting, landing
# and disk check at whichever checkout ran setup.sh LAST". deploy/ci.sh runs
# setup.sh from a per-checkout worktree and `gt done` DELETES that worktree.
# Observed on the deployment box 2026-08-09: running `deploy/setup.sh
# vault-unitfix --test` from a polecat worktree left prod's units executing
# .../polecats/vault/dumpstar/deploy/alert.sh, which is 203/EXEC the moment
# the branch lands — the exact shape of the Jun 2026 backup outage, where the
# unit was installed, enabled, and executing nothing. Prod only stayed alarmed
# because a later prod-clone setup.sh happened to win the race.
#
# They are therefore written only by an install ENTITLED to them:
#
#   * one for an instance this box treats as a REAL DEPLOYMENT
#     (dumpstar_units_alerting — `prod`, or whatever DUMPSTAR_ALERT_INSTANCES
#     names). That predicate already decides whether an instance's units may
#     page; "may this instance own the pager" and "may this instance own the
#     unit the pager lives in" are the same question from two sides, so they get
#     one answer rather than two that can drift.
#   * anyone, when NOBODY OWNS THEM YET. A box with none installed needs
#     somebody to install them, and a dev box may never have a `prod` at all.
#   * anyone, when the owner's checkout IS GONE. Those units are already broken;
#     pointing them at a directory that exists is a repair, not a theft.
#   * DUMPSTAR_INSTALL_HOST_UNITS=1, the explicit override. 0 forces the skip.
#
# Refusing is a SKIP, not a fatal error: the per-instance units are still
# installed, because a CI gate standing up its own throwaway instance is the
# normal case here, not the anomaly. It is loud on stdout, naming the owner.
#
# The owner is read back OUT OF THE INSTALLED UNITS rather than kept in a marker
# file beside them, for the reason deploy/store-lib.sh gives about the Quadlet:
# THE UNIT IS THE RECORD. A marker can disagree with what systemd will execute;
# an ExecStart cannot.

# dumpstar_systemd_user_dir — where the host-wide units live. The invoking
# user's real directory unless DUMPSTAR_SYSTEMD_USER_DIR names another: a test or
# build installs into a scratch directory, never the box's own (sp-9e2sn9 — two
# installs from aeons on the Spira host left 23, then 27, unit files in the
# operator's real ~/.config/systemd/user).
dumpstar_systemd_user_dir() {
    printf '%s\n' "${DUMPSTAR_SYSTEMD_USER_DIR:-${HOME}/.config/systemd/user}"
}

# dumpstar_units_real_dir — is the directory the real one for this user?
dumpstar_units_real_dir() {
    [ "$(dumpstar_systemd_user_dir)" = "${HOME}/.config/systemd/user" ]
}

# Every unit file that is one-per-box. tests/deploy/run.sh §16 asserts this list
# covers the directory in BOTH directions — an install writing a shared unit
# that is not declared here would be unguarded, and a name declared here that
# nothing writes would make the owner probe read a stale file forever.
DUMPSTAR_HOST_WIDE_UNIT_FILES=(
    dumpstar-refresh@.service          dumpstar-refresh@.timer
    dumpstar-derive@.service           dumpstar-derive@.timer
    dumpstar-catalog-sync@.service     dumpstar-catalog-sync@.timer
    dumpstar-catalog-check@.service    dumpstar-catalog-check@.timer
    dumpstar-value-snapshots@.service  dumpstar-value-snapshots@.timer
    dumpstar-ship@.service             dumpstar-ship@.timer
    dumpstar-backup-check@.service     dumpstar-backup-check@.timer
    dumpstar-alert@.service
    dumpstar-diskcheck.service         dumpstar-diskcheck.timer
    dumpstar-tmpreap.service           dumpstar-tmpreap.timer
    dumpstar-heartbeat@.service        dumpstar-heartbeat@.timer
)

# dumpstar_units_host_owners — every checkout the INSTALLED host-wide units
# resolve their ExecStart against, one absolute path per line, deduplicated.
# Empty output means no host-wide unit is installed (or none names a script,
# which is true of every .timer).
dumpstar_units_host_owners() {
    local dir u
    dir="$(dumpstar_systemd_user_dir)"
    for u in "${DUMPSTAR_HOST_WIDE_UNIT_FILES[@]}"; do
        [ -f "${dir}/${u}" ] || continue
        grep -oh "/[^ '\"]*/deploy/[A-Za-z0-9_.-]*\.sh" "${dir}/${u}" 2>/dev/null
    done | sed 's|/deploy/[^/]*$||' | sort -u
}

# The checkout dumpstar_units_host_entitled refused in favour of. Empty otherwise.
DUMPSTAR_UNITS_HOST_OWNER=""
DUMPSTAR_UNITS_HOST_SKIP_REASON=""

# dumpstar_units_host_entitled <instance> — may this install rewrite the units
# there is one of per box? See the header for the four ways to be entitled.
dumpstar_units_host_entitled() {
    local instance="$1" repo_dir owner
    repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    DUMPSTAR_UNITS_HOST_OWNER=""
    DUMPSTAR_UNITS_HOST_SKIP_REASON=""

    case "${DUMPSTAR_INSTALL_HOST_UNITS:-}" in
        1) return 0 ;;
        0) return 1 ;;
    esac

    # A test install (setup.sh --test sets DUMPSTAR_UNITS_TEST=1) never writes the
    # box's real unit directory unless it is entitled by being a real deployment
    # or forced above. Point DUMPSTAR_SYSTEMD_USER_DIR at a scratch directory and
    # it installs there as usual (sp-9e2sn9).
    if [ "${DUMPSTAR_UNITS_TEST:-}" = 1 ] && dumpstar_units_real_dir \
        && ! dumpstar_units_alerting "$instance"; then
        DUMPSTAR_UNITS_HOST_SKIP_REASON="a test install does not write the real unit directory (set DUMPSTAR_SYSTEMD_USER_DIR to a scratch directory)"
        return 1
    fi

    dumpstar_units_alerting "$instance" && return 0

    while read -r owner; do
        [ -n "$owner" ] || continue
        [ "$owner" = "$repo_dir" ] && continue
        # A checkout that is gone cannot be robbed, and its units are already
        # executing nothing. Taking them over is the repair.
        [ -d "$owner" ] || continue
        DUMPSTAR_UNITS_HOST_OWNER="$owner"
        return 1
    done < <(dumpstar_units_host_owners)

    return 0
}

# _dumpstar_units_render <dest> <stamp|nostamp> <template> [sed -e args...]
#
# Render a template to <dest>, but only replace the file when the result
# differs — so an unchanged unit keeps its mtime and the changed ones are the
# ones reported. Written via a dot-prefixed sibling and moved into place: a
# torn write in the Quadlet directory is a unit systemd cannot parse, and the
# dot prefix keeps a leftover out of the *.container glob.
_dumpstar_units_render() {
    local dest="$1" stamp="$2" template="$3"
    shift 3
    local dir base tmp
    dir="$(dirname "$dest")"
    base="$(basename "$dest")"
    tmp="${dir}/.${base}.new.$$"

    sed "$@" "$template" > "$tmp"

    if [ "$stamp" = stamp ]; then
        # systemd does not inherit our PATH, so the unit carries the store flags
        # itself. A no-op without an active alternate store — which is prod.
        dumpstar_store_stamp_unit "$tmp"
    fi

    if [ -f "$dest" ] && cmp -s "$tmp" "$dest"; then
        rm -f "$tmp"
    else
        mv "$tmp" "$dest"
        DUMPSTAR_UNITS_CHANGED+=("$base")
    fi
}

# Host-wide units this checkout no longer ships. The Python lake went with
# sp-l74uk0 and took deploy/prices.sh with it, but a box that installed the
# units before then kept dumpstar-prices@.service: its nightly run exec'd a
# script that no longer exists and failed every night, and its OnFailure alert
# failed with it (sp-104cjp). An install entitled to the host-wide units
# retires them, so the unit directory is a copy of what the checkout ships.
DUMPSTAR_RETIRED_HOST_UNIT_FILES=(
    dumpstar-prices@.service           dumpstar-prices@.timer
)

# _dumpstar_units_retire_stale <systemd_user_dir>: stop, remove and clear the
# failed state of every retired unit that is still installed, naming each in
# DUMPSTAR_UNITS_CHANGED. systemctl is touched only for the real directory.
_dumpstar_units_retire_stale() {
    local dir="$1" u inst
    for u in "${DUMPSTAR_RETIRED_HOST_UNIT_FILES[@]}"; do
        [ -e "${dir}/${u}" ] || [ -L "${dir}/${u}" ] || continue
        if dumpstar_units_real_dir && command -v systemctl >/dev/null 2>&1; then
            for inst in $(systemctl --user list-units --all --plain --no-legend \
                "${u%%@*}@*.${u##*.}" 2>/dev/null | awk '{print $1}'); do
                systemctl --user disable --now "$inst" >/dev/null 2>&1 || true
                systemctl --user reset-failed "$inst" "dumpstar-alert@${inst}.service" >/dev/null 2>&1 || true
            done
        fi
        rm -f "${dir}/${u}"
        rm -f "${dir}"/*.wants/"${u%%@*}@"*."${u##*.}" 2>/dev/null || true
        DUMPSTAR_UNITS_CHANGED+=("${u} (removed)")
    done
}

# _dumpstar_units_install_host_wide <repo_dir> <systemd_user_dir>
#
# The units there is ONE OF per box. See "HOST-WIDE UNITS" above for who may
# write them and why that is not everybody.
_dumpstar_units_install_host_wide() {
    local repo_dir="$1" systemd_user_dir="$2"
    local ext

    _dumpstar_units_retire_stale "$systemd_user_dir"

    # --- The nightly landing run (pd-1uem item 5, pd-lunn) ------------------
    # A %i TEMPLATE, which is one file backing every instance — see the header
    # above. Enable it per instance after setup if desired. {{REPO_DIR}}
    # resolves ExecStart against THIS checkout (single-clone deployment — the
    # clone is not named per-instance).
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-refresh@.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-refresh.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done

    # --- The offline catalog derive (pd-1uem, pd-lunn) ----------------------
    # The DERIVING half of the nightly pair; dumpstar-refresh@ above is the
    # landing half. Since pd-lunn it is the ONLY thing that builds
    # shared.sqlite, so on a box that runs the refresh this one is not
    # optional — deploy/refresh.sh refuses to fetch anything while it is
    # disabled. Still installed rather than enabled here: arming a timer is an
    # operator's act on an instance, and the unit's ConditionPathExists on
    # lake.env keeps an enabled timer inert on a box that has no landing zone.
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-derive@.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-derive.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done

    # --- The SWU catalog sync and its alarm check (dumpstar 7) --------------
    # Installed for every instance, enabled for none: arming a timer that
    # fetches from two upstreams is an operator's act on an instance.
    for unit in catalog-sync catalog-check value-snapshots; do
        for ext in service timer; do
            _dumpstar_units_render "${systemd_user_dir}/dumpstar-${unit}@.${ext}" nostamp \
                "$repo_dir/deploy/dumpstar-${unit}.${ext}" \
                -e "s|{{REPO_DIR}}|${repo_dir}|g"
        done
    done

    # --- The ownership shipment (pd-dxn3) -----------------------------------
    # The outbox into the tenant zone, ordered after the transform above
    # because both open every tenant's database. Installed for every instance
    # and enabled for none: arming it is an operator's deliberate act.
    # (setup-lake.sh --arm-shipper went with the Python lake, sp-l74uk0.)
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-ship@.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-ship.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done

    # --- Backup-failure alarming units (dumpstar-ivq) -------------------
    # Layer 1: backup-freshness dead-man's switch (per-instance @ template).
    # Layer 2: OnFailure -> Pushover journal-tail push (instance-by-failed-unit).
    # Layer 4: host-wide low-disk check (not per-instance).
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-backup-check@.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-backup-check.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done
    _dumpstar_units_render "${systemd_user_dir}/dumpstar-alert@.service" nostamp \
        "$repo_dir/deploy/dumpstar-alert@.service" \
        -e "s|{{REPO_DIR}}|${repo_dir}|g"
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-diskcheck.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-diskcheck.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done

    # Layer 4b: the step before the low-disk check (pd-xgh6). diskcheck says the
    # disk is filling; this is what stops it filling. Host-wide for the same
    # reason and not per-instance either — what it collects is every agent's
    # abandoned session scratchpads under $TMPDIR, which is the filesystem
    # pd-20ia taught deploy/ci.sh to measure and which blocked CI here at 99%
    # full while / had 40G free. Installed, not enabled: arming a timer that
    # deletes directories is an operator's act, and only a checkout entitled to
    # the host-wide units gets this far at all.
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-tmpreap.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-tmpreap.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done

    # Layer 0: is the site serving at all? Numbered below layer 1 because it is
    # the question the other layers assume the answer to. Backup freshness only
    # tells you about the box INDIRECTLY, on a 6h period with a 3h grace; on
    # 2026-08-16 the site was hard down and nothing paged, because the only
    # liveness signal was that proxy and it had already been failing for four
    # days over an unrelated bug.
    for ext in service timer; do
        _dumpstar_units_render "${systemd_user_dir}/dumpstar-heartbeat@.${ext}" nostamp \
            "$repo_dir/deploy/dumpstar-heartbeat.${ext}" \
            -e "s|{{REPO_DIR}}|${repo_dir}|g"
    done
}

# dumpstar_units_install <instance> [port]
#
# (Re)install every unit file this checkout ships for <instance>. Idempotent.
# The caller reloads systemd afterwards — this function only writes files.
#
# port: the host port to publish, or 0 / omitted for "decide". Deciding means
# the port the instance ALREADY publishes if it has one, and otherwise ":8080"
# so Podman picks a free one. Refreshing the units must never move a running
# instance's address: without that, `deploy.sh prod` would rewrite prod's
# Quadlet with a random host port and take prod off 8090 — an outage caused by
# shipping a unit-file fix.
dumpstar_units_install() {
    local _alert_sed=()
    dumpstar_units_alerting "$1" || _alert_sed=(-e '/^OnFailure=/d')
    local instance="$1" port="${2:-0}"
    local repo_dir quadlet_dir systemd_user_dir service_name
    repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    quadlet_dir="${HOME}/.config/containers/systemd"
    systemd_user_dir="$(dumpstar_systemd_user_dir)"
    service_name="dumpstar-${instance}"

    DUMPSTAR_UNITS_CHANGED=()
    mkdir -p "$quadlet_dir" "$systemd_user_dir"

    local existing_port port_mapping
    if [ "$port" = "0" ] && [ -f "${quadlet_dir}/${service_name}.container" ]; then
        existing_port="$(sed -n \
            -e 's|^PublishPort=\([0-9]\{1,\}\):8080$|\1|p' \
            -e 's|^PublishPort=127\.0\.0\.1:\([0-9]\{1,\}\):8080$|\1|p' \
            "${quadlet_dir}/${service_name}.container" | head -1)"
        if [ -n "$existing_port" ]; then
            port="$existing_port"
            echo "    Keeping the port '${instance}' already publishes: ${port}"
        fi
    fi
    # PORT=0 -> ":8080" lets Podman pick a free host port.
    # prod's origin port is fixed (sp-kaj3xv): the tunnel route points at it, so it
    # must be the same on every box and every re-creation. 8090 is the other site's.
    if [ "$port" = "0" ] && [ "$instance" = prod ]; then
        port="$DUMPSTAR_PROD_PORT"
    fi
    if [ "$port" = "0" ]; then
        port_mapping=":8080"
    else
        port_mapping="${port}:8080"
    fi

    # --- The app's Quadlet unit ---------------------------------------------
    _dumpstar_units_render "${quadlet_dir}/${service_name}.container" stamp \
        "$repo_dir/deploy/dumpstar.container" \
        -e "s|{{INSTANCE}}|${instance}|g" \
        -e "s|{{PORT}}:8080|${port_mapping}|g" \
        "${_alert_sed[@]}"

    # --- The host-wide units ------------------------------------------------
    # One copy of each on the box, shared by every instance. Written only by an
    # install entitled to them (pd-onyd) — see the header.
    if dumpstar_units_host_entitled "$instance"; then
        _dumpstar_units_install_host_wide "$repo_dir" "$systemd_user_dir"
    elif [ -n "$DUMPSTAR_UNITS_HOST_OWNER" ]; then
        echo "    Host-wide units NOT rewritten — they belong to ${DUMPSTAR_UNITS_HOST_OWNER}."
        echo "      There is one copy of each on this box, shared by every instance"
        echo "      including prod, and this checkout is not the one that deploys it."
        echo "      Force with DUMPSTAR_INSTALL_HOST_UNITS=1 (pd-onyd)."
    else
        echo "    Host-wide units NOT rewritten — ${DUMPSTAR_UNITS_HOST_SKIP_REASON:-DUMPSTAR_INSTALL_HOST_UNITS=0}."
    fi

    # --- The Litestream backup sidecar (dumpstar-8ch.3) -----------------
    _dumpstar_units_render "${quadlet_dir}/dumpstar-litestream-${instance}.container" stamp \
        "$repo_dir/deploy/dumpstar-litestream.container" \
        -e "s|{{INSTANCE}}|${instance}|g" \
        -e "s|{{REPO_DIR}}|${repo_dir}|g" \
        "${_alert_sed[@]}"
}

# dumpstar_units_report — say what the last install actually changed.
#
# The point of naming them: unit drift is invisible by construction (the
# installed copy and the template are different files in different trees), so
# the deploy that finally corrects months of it should say so out loud rather
# than look exactly like the deploy that corrected nothing.
dumpstar_units_report() {
    if [ ${#DUMPSTAR_UNITS_CHANGED[@]} -eq 0 ]; then
        echo "    Unit files already match this checkout."
    else
        echo "    Unit files updated from this checkout (${#DUMPSTAR_UNITS_CHANGED[@]}):"
        printf '      %s\n' "${DUMPSTAR_UNITS_CHANGED[@]}"
    fi
}
