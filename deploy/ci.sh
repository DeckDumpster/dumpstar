#!/usr/bin/env bash
#
# Local CI loop for Dumpstar — the "super tight" inner dev cycle.
#
# Reproduces what a GitHub `ci.yml` workflow would do, as a plain script so
# it runs identically on a laptop and (eventually) on a CI runner. A future
# GitHub workflow should be a thin wrapper that just calls this file.
#
# Steps:
#   1. Tear down any stale container instance belonging to THIS checkout.
#  1b. Harness gate:  prove the shell harnesses can describe their own failure,
#                     that no harness picks a host port instead of asking the
#                     kernel, that they wait for conditions rather than for the
#                     clock, that the Containerfile's base images are pinned to
#                     a Debian release, that a Layer 2 page leads with the
#                     line that says what went wrong, that an unchanged one
#                     is not sent a second time, and that a run whose worktree
#                     is rebased underneath it goes VOID rather than reporting
#                     on a tree that never existed. Hermetic and sub-second.
#                     See tests/lib/diagnostics_test.sh, tests/lib/ports_test.sh,
#                     tests/lib/wait_test.sh, tests/lib/objects_test.sh,
#                     tests/lib/netrun_test.sh,
#                     tests/lib/litestream_test.sh,
#                     tests/lib/images_test.sh,
#                     tests/container/base_images_test.sh,
#                     tests/visual/approval_test.sh,
#                     tests/litestream/image_pin_test.sh,
#                     tests/litestream/seeded_fault_test.sh,
#                     tests/alarming/journal_summary_test.sh,
#                     tests/alarming/alert_suppress_test.sh,
#                     tests/ci/treewatch_test.sh and
#                     tests/ci/unit_diagnostics_test.sh.
#   2. Rust gates:     cargo fmt --check, cargo clippy --all-targets, frontend
#                      check, then cargo test (cheapest-first; sp-l2xovg).
#   3. Frontend gate:  npm ci && npm test && npm run check && npm run build.
#  3b. The image:      built ONCE, here. Five gates below need the shipped image
#                      and each wants its own tag; they tag this one rather than
#                      each running a builder over identical content. See
#                      deploy/image-lib.sh.
#   4. Container gate: build + start a `--test` instance, wait for the server
#                      to answer on its port, then tear it down.
#   5. Backup gate:    replicate four tenant databases through the SHIPPED
#                      deploy/litestream.yml to a throwaway MinIO and restore a
#                      NON-FIRST one, asserting it comes back as itself. See
#                      tests/litestream/run.sh.
#   6. DR drill:       run deploy/RESTORE.md's procedure with the shipped
#                      scripts — restore one tenant in place while the others
#                      keep exactly their own data, then walk the recovery matrix: after
#                      a RENAME, of a DETACHED tenant, and the load-bearing
#                      negative — restoring the tenant files WITHOUT the registry
#                      must FAIL, shown succeeding-and-anonymous first. See
#                      tests/litestream/drill.sh.
#  6b. Alarming gate:  make every backup-alarming layer FIRE at a local sink
#                      standing in for healthchecks.io + Pushover, and assert on
#                      what it sent. See tests/alarming/run.sh.
#   7. Recreate proof: create a user, remove her, create her again, and prove no
#                      restore of the second one can reach the first one's card
#                      — pd-pm7b, closed executably. See tests/litestream/recreate.sh.
#   8. Upgrade gate:   start the SHIPPED image against a data volume built in
#                      the OLD layout, migrate it onto opaque database ids, roll
#                      that back, and assert it serves the same collection at
#                      every step. See tests/tenants/upgrade.sh.
#   9. Handle gate:    start the SHIPPED image with tenant resolution ON and
#                      assert what it answers to a tenant header: malformed is
#                      400, unknown is 404, and single-tenant mode does not read
#                      the header at all. See tests/tenants/handles.sh.
#  9b. Key custody:    mint the tenant-zone master key with the SHIPPED binary
#                      and stat the file ON THE HOST — "the code sets 600" and
#                      "the file is 600" are different claims. Then the property
#                      the whole item turns on, on a real box: with the master
#                      key moved away, a tombstoned tenant still reads as
#                      REVOKED while a live one reads as an OPERATIONAL failure.
#                      A lost key never reports as a deleted tenant. See
#                      tests/keys/run.sh.
#  10. Browser gate:   screenshot every route at 1440 and 768 against that
#                      same instance and diff against the committed baselines,
#                      and assert the DOM bounds that a screenshot cannot see —
#                      /collection renders a viewport-sized WINDOW of a
#                      56,635-row result, with no pager anywhere and the far
#                      end a scroll away. See tests/visual/README.md.
#  11. Schema-version gate: start a prod-shaped instance against a deliberately
#                      UNVERSIONED data volume — the shape every database on
#                      disk has, prod's included — and assert every database is
#                      adopted and serves; then assert one from the future is
#                      refused. See tests/schema-version/run.sh.
#  12. Lakehouse gate: stand up Nessie on a durable ROCKSDB store plus a
#                      throwaway MinIO, and drive the shipped PyIceberg job
#                      through write -> read -> commit -> TIME TRAVEL, then
#                      restart the catalog and re-read. Time travel is the only
#                      reason Iceberg + Nessie is here, and a stack that has
#                      lost it looks perfectly healthy. See tests/lake/run.sh.
#  13. Table gate:     build catalog.prices from the raw landing zone on a
#                      podman --internal network — no route to any upstream,
#                      asserted rather than assumed — and compare it row for
#                      row against the shared.sqlite built from the SAME
#                      upstream bytes. See tests/lake/prices.sh. And the same
#                      claim for the whole CATALOG: shared.sqlite derived from
#                      raw/ by the shipped image with egress provably blocked,
#                      row-identical to a catalog built by fetching. See
#                      tests/lake/derive.sh.
#  14. Transform gate: value snapshots for EVERY registered tenant, computed
#                      from that table. Byte-identical to what Rust's
#                      snapshot_today produces for the same tenant and date —
#                      and since pd-i08u that is asserted THROUGH the tenant
#                      zone, because the online holdings read is gone and §4b
#                      has to ship and read back before the transform has
#                      anything to value. A second tenant who never had a
#                      snapshot row gets one (pd-s5yn inverted), and a tenant
#                      nobody can write to is skipped with the run exiting 2
#                      rather than 0. §10 then drives the SHIPPED
#                      deploy/value-snapshots.sh — the file the nightly timer
#                      executes — over the same lake. See
#                      tests/lake/value_snapshots.sh.
#  14b.Tenant-zone gate: the tenant zone shares the lake's bucket and is
#                      separated from it by a prefix — which is to say by two
#                      credential policies and a lifecycle rule, and by nothing
#                      else. Assert the boundary in BOTH directions against a
#                      MinIO, then BREAK it deliberately and assert the same
#                      checks go red; assert the 90-day retention is mechanical
#                      and that no rule can reach raw/, whose retention is
#                      indefinite by decision. See tests/lake/tenant_zone.sh.
#  14e.Phase 3 gate:   a collection valued from the TENANT ZONE, which since
#                      pd-i08u is the only way to value one — §5 also asserts
#                      that neither --holdings nor --compare is still
#                      reachable. §6 is the section that matters: a collection
#                      changed WITHOUT shipping must leave the valuation
#                      UNMOVED, and §6b requires shipping and reading back to
#                      move it. A Phase 3 that quietly read the live table
#                      fails the first half; one that is frozen or cached fails
#                      the second. See tests/lake/phase3.sh.
#  14f.Deletion gate:  a tenant erased from the zone, and PROVEN erased —
#                      against a VERSIONED bucket, so the drop leaves a
#                      noncurrent version of a really-shipped part behind and
#                      that surviving copy is fetched back and proven
#                      unopenable. Every check is run one step EARLIER too and
#                      required to report the path OPEN: a proof only ever seen
#                      passing is not known to prove anything. See
#                      tests/lake/deletion.sh.
#  15. Refresh gate:   the other half of that — a real `dumpstar data refresh`,
#                      through the shipped image, over a data directory with
#                      two provisioned tenants in it, must leave every tenant
#                      database byte-identical. Since pd-lunn the refresh
#                      LANDS and builds nothing, so shared.sqlite has to come
#                      out byte-identical too: the same gate now covers item
#                      6's acceptance criterion against the shipped binary.
#                      See tests/refresh/tenant_bytes.sh.
#
# Steps 5-9b and 11-15 do not run where they are written. Each one QUEUES itself
# under its own tier guard and they are run together, two at a time, by step
# 16 — see the "PARALLEL GATES" note below and deploy/ci-parallel.sh.
#
# The intents UI harness (tests/ui) is deliberately NOT part of this loop:
# until the replay implementations are generated it needs an ANTHROPIC_API_KEY
# for Vision mode, which makes it slow and non-deterministic. (The browser gate
# in step 10 also drives Playwright, but offline and deterministically — that is
# the difference, not the browser.) Run the intents harness on its own:
#   (cd tests/ui && npx playwright install chromium && npx playwright test)
#
# Exits non-zero on the first failure of the sequential steps. Fast and
# re-runnable.
#
# EXIT 9 IS NEITHER PASS NOR FAIL (pd-vnbc). This script watches its own
# checkout and voids the run if anything moves HEAD while it is going — a
# rebase, a checkout, a reset, a stash, from any process. It happened: a
# polecat's worktree was rebased onto origin/master mid-suite and clippy
# compiled the half-replayed tree, producing seven errors naming symbols that
# exist in no commit. The rebase aborted a second later, leaving nothing to
# find. A 9 means "this result describes no state this code was ever in" —
# go and find what touched the checkout, then re-run from the top. See
# deploy/ci-treewatch.sh and tests/ci/treewatch_test.sh.
#
# PARALLEL GATES (pd-2nl9). Fourteen of the steps above stand up their own
# containers and share nothing — every name each of them uses is derived from
# its own prefix plus a per-checkout hash, because concurrent polecats already
# run whole suites of this script beside each other. So they are queued rather
# than run in place, and step 16 runs them two at a time. The cap, the disk
# floor checked before every dispatch, and why a failure in one is never masked
# by the others in its wave, are all in deploy/ci-parallel.sh.
#
# Two consequences worth knowing before reading a log:
#
#   * a parallel gate's output is printed in ONE block when that gate finishes,
#     not as it happens, and the blocks come out in completion order;
#   * a failing parallel gate no longer stops the ones beside it. The wave
#     finishes, every gate's output is printed, and the run ends red naming all
#     of them. One red run now tells you everything that is broken.
#
#   DUMPSTAR_CI_JOBS=1 bash deploy/ci.sh   # run them one at a time instead
#
# TIER SELECTION (pd-s2mj). Every step above is a named tier, and a caller may
# hand over the list of paths a change touched; the tiers those paths cannot
# affect are then reported as skipped instead of run. It is opt-in per run and
# it fails closed — no list means EVERY tier, and an unrecognised path means
# every tier. .github/workflows/ci.yml passes a list for PULL REQUESTS ONLY, so
# a developer, a polecat, `workflow_dispatch` and any push-triggered run all
# get the full suite by construction. The rule lives in deploy/ci-select.sh and
# tests/ci/select_test.sh asserts it, including that the names below and the
# names there have not drifted apart.
#
# Usage:
#   bash deploy/ci.sh
#   DUMPSTAR_CI_INSTANCE=myname bash deploy/ci.sh   # pin the instance name
#   DUMPSTAR_STORE_ROOT=/some/dir bash deploy/ci.sh # pin the container store
#   DUMPSTAR_STORE_ROOT= bash deploy/ci.sh          # use Podman's default store
#                                                 # (overrides host store.env)
#   DUMPSTAR_CI_CHANGED_FILES=list bash deploy/ci.sh  # select tiers from a list
#                                                 # of changed paths
#   DUMPSTAR_CI_SELECT_ONLY=1 bash deploy/ci.sh     # print the plan, run nothing
#   DUMPSTAR_CI_JOBS=1 bash deploy/ci.sh            # how many container gates run
#                                                 # at once (default 3, max 4)
#   DUMPSTAR_PREBUILT_IMAGE=localhost/dumpstar:x bash deploy/ci.sh
#                                                 # skip even the one build and
#                                                 # test that image instead
#
# Parallel-safe: the container instance is named per-checkout, so several
# polecats can run this concurrently from their own worktrees without tearing
# down each other's containers. Do not reintroduce a fixed instance name.
#
# Disk: nothing this script builds belongs on the disk prod runs from. Point
# DUMPSTAR_STORE_ROOT at another filesystem and the whole container store — images,
# layers, volumes and Buildah's cache mounts — goes there instead. Which disk
# that is on a given box is host config, not a repo constant, so it is read from
# ~/.config/dumpstar/store.env; unconfigured, Podman's default store is used. See
# deploy/store-lib.sh. This script also refuses to start on a nearly-full disk,
# because the failure that produces does not look like a disk problem.
#
# A second store is not free: podman 4.9 lets one store's cleanup take the
# rootless-netns state the other one is holding, after which every container on
# a user-defined network in that store fails — steps 5, 6 and 6b create one, so
# the whole run does. dumpstar_store_activate detects and repairs that below; the
# mechanism is in deploy/store-lib.sh (pd-yfev). `deploy/store-teardown.sh`
# removes a store outright, which nothing did before.
#
set -euo pipefail

# systemctl --user / podman need XDG_RUNTIME_DIR; CI runners and
# non-interactive shells often lack it.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

# TMPDIR MUST NOT BE A RAM DISK.
#
# On a systemd distribution /tmp is a tmpfs sized at half of RAM. This suite
# writes gigabytes through it — cargo's link staging, podman's build context,
# every harness fixture — and the disk floor below runs against whatever
# directory it was handed, so on a 8G VM the whole run died at:
#
#     ERROR: only 4G free on /tmp (floor 10G).
#     tmpfs  3.7G  400K  3.7G  1% /tmp
#
# which is not a disk problem with the machine: / had 79G free at the time.
#
# It never fired on the old runner because that box had a disk-backed /tmp.
# Nothing in the repository said so, which is the whole shape of this class of
# bug: a property of one machine that the suite silently depended on.
# deckdumpster hit exactly this on its first ephemeral VM (de-323) and fixed it
# the same way. The durable fix is the Proxmox template giving its clones a
# disk-backed /tmp; this holds regardless, and also covers a developer's laptop.
#
# Set here, before DUMPSTAR_CI_DISK_PATHS is built, so the floor measures the
# directory the run will actually write to.
#
# AHEAD OF THE TREE WATCH, and it has to be: diag_init below mktemps its error
# capture under $TMPDIR, and the watch's own failure path calls diag, so the
# order TMPDIR -> diagnostics -> treewatch is a real dependency chain rather
# than an accident of layout.
#
# Which is why the notice below is NOT a `==> ` step line. tests/ci/treewatch_test.sh
# §9 asserts the watch is announced ahead of every step, by executing a real run
# and taking the first of `treewatch: armed on|^==> `, and this would be first —
# so it is indented like the watch's own banner, because that is what it is:
# setup that must precede the watch, not a step running unwatched. The
# distinction is load-bearing and it is not cosmetic, so do not restore the
# arrow. It only ever showed up on a box whose /tmp is a tmpfs; the CI VM's is
# disk-backed, so the gate went red on a developer's machine and green on the
# runner — which is the wrong way round for a guard about unwatched work.
case "$(stat -f -c %T "${TMPDIR:-/tmp}" 2>/dev/null)" in
    tmpfs|ramfs)
        TMPDIR="${HOME}/.cache/dumpstar-tmp"
        mkdir -p "$TMPDIR"
        export TMPDIR
        echo "    TMPDIR moved to $TMPDIR (/tmp is a RAM disk)"
        ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Share one cargo target directory across aeon builds (db-jz5t). Without this,
# each worktree defaults to its own <worktree>/target, so N aeons in flight means
# N independent 5G copies of the same build output. The gate already does this
# with a separate path ($HOME/.cache/spira-gate/dumpstar); this gives aeon
# invocations the same property.
#
# Cargo serialises concurrent builds via its own file lock. That converts a disk
# problem into latency: a second aeon blocks while the first compiles. At
# SPIRA_MAX_AEONS pool size that trade is clearly worth it — incremental builds
# are seconds, and contention is occasional. A caller that wants the worktree dir
# (e.g. an isolated test fixture) can override by setting CARGO_TARGET_DIR before
# invoking this script.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/dumpstar-builds}"

# Name our own failure (pd-8gjs). A gate that dies without printing why used to
# leave the EXIT trap's "Stopping dumpstar-ci-..." as the last line in the log,
# which reads like a clean shutdown and says nothing about the step that died.
# shellcheck source=tests/lib/diagnostics.sh
. "${REPO_DIR}/tests/lib/diagnostics.sh"
diag_init

# Bounded condition polling, in one place for every harness (pd-86er).
# shellcheck source=tests/lib/wait.sh
. "${REPO_DIR}/tests/lib/wait.sh"

# Void a run whose checkout moves underneath it (pd-vnbc). Something outside
# this repository rebased a live polecat worktree mid-suite, and clippy
# compiled the half-replayed tree: seven errors naming symbols that exist in no
# commit, on a tree that passes clean. The rebase aborted a second later, so
# nothing was left to find and the whole thing read as a compiler phantom.
# Armed here, before the first step, because it is the steps that ALREADY RAN
# whose results a later mutation invalidates. See deploy/ci-treewatch.sh.
# shellcheck source=deploy/ci-treewatch.sh
. "$SCRIPT_DIR/ci-treewatch.sh"
dumpstar_treewatch_begin || exit "$DUMPSTAR_TREEWATCH_EXIT"

# The instance name has to be unique per checkout. The swarm runs several
# polecats per rig, each from its own worktree, and every one of them runs
# this script; with a shared name, run B's opening teardown destroyed run A's
# container mid-suite (observed 2026-08-03, polecats pipboy and raider). The
# symptoms surfaced as screenshot instability in unrelated routes, so this
# also cost debugging time somewhere else entirely.
#
# Derived from the full worktree path, not its basename: every polecat
# worktree is .../polecats/<name>/dumpstar, so a basename would collide
# for all of them. Hashing the whole path keeps the name stable per checkout
# — so the stale-cleanup below still finds the previous run's leftovers —
# while making a cross-checkout collision impossible.
INSTANCE="${DUMPSTAR_CI_INSTANCE:-ci-$(printf '%s' "$REPO_DIR" | sha1sum | cut -c1-8)}"
SERVICE_NAME="dumpstar-${INSTANCE}"
CONTAINER="systemd-${SERVICE_NAME}"

# The one image this run builds. Five gates need it and each wants its own tag,
# so it is built once under this name and every gate tags it — per-checkout for
# the same reason the instance name is, since concurrent polecats share a store.
BUILD_IMAGE="localhost/dumpstar:build-${INSTANCE}"

START_TIME=$(date +%s)

CURRENT_STEP="startup"
# Every step boundary is a checkpoint for the worktree watch. The check is a
# `wc -c` on one file, so it is affordable at this frequency, and the frequency
# is what turns "the tree moved" into "the tree moved during step N" — a named
# window instead of a forty-minute one. It aborts rather than warning: once the
# tree has moved, the gates behind us are the ones whose verdicts are void, and
# running the remaining thirty minutes cannot recover them.
step() {
    dumpstar_treewatch_check "at the start of step: $*" || exit "$DUMPSTAR_TREEWATCH_EXIT"
    CURRENT_STEP="$*"
    echo ""
    echo "==> $*"
}

# Capture the unit journal before a teardown destroys it. Shared with
# deploy/deploy.sh — one definition in one place (db-g6ku).
# shellcheck source=deploy/diagnostics-lib.sh
. "$SCRIPT_DIR/diagnostics-lib.sh"

# --- 0. Tier selection -------------------------------------------------------
#
# EVERY TIER RUNS unless a caller hands over an explicit list of changed paths
# or an explicit list of tier names. A developer, a polecat, `workflow_dispatch`,
# and any push-triggered run therefore get the full suite by construction —
# skipping a tier takes an affirmative act by the caller, and the act is
# printed in the log below.
#
#   DUMPSTAR_CI_TIERS=<names>          space-separated list of tier names to run,
#                                    unioned with the always-on tiers (lint).
#                                    Spira's per-branch gate sets this. An
#                                    unknown name is a hard error — never a
#                                    silent fallback to the full suite, because
#                                    this variable SUBTRACTS and a typo that
#                                    ran everything would hide itself forever.
#                                    Cannot be set alongside DUMPSTAR_CI_CHANGED_FILES.
#   DUMPSTAR_CI_CHANGED_FILES=<file>   one changed path per line; the tiers those
#                                    paths require are run and the rest are
#                                    reported as skipped. .github/workflows/
#                                    ci.yml sets this for PULL REQUESTS ONLY.
#   DUMPSTAR_CI_SELECT_ONLY=1          print the plan and exit 0, touching
#                                    nothing. This is how tests/ci/select_test.sh
#                                    asserts on the real script rather than on a
#                                    copy of its rules.
#
# The rule itself, and why it fails closed, is deploy/ci-select.sh. Nothing
# about WHICH tiers exist lives in two places: the names below are checked
# against that file's canonical list by tests/ci/select_test.sh.
#
# shellcheck source=deploy/ci-select.sh
. "$SCRIPT_DIR/ci-select.sh"

if [ -n "${DUMPSTAR_CI_TIERS:-}" ] && [ -n "${DUMPSTAR_CI_CHANGED_FILES:-}" ]; then
    echo "ERROR: DUMPSTAR_CI_TIERS and DUMPSTAR_CI_CHANGED_FILES are both set — a caller that sets one must not set the other." >&2
    exit 1
elif [ -n "${DUMPSTAR_CI_TIERS:-}" ]; then
    # Explicit tier list from a caller (e.g. Spira's per-branch gate). Each name
    # is validated against the canonical list — an unknown name is a hard error,
    # not a silent fallback to the full suite, which would hide a typo forever.
    #
    # LITERAL, NO FLOOR. DUMPSTAR_CI_ALWAYS_TIERS is a floor under path-based
    # selection (DUMPSTAR_CI_CHANGED_FILES), where the input is a diff and the
    # caller has stated no intent. An explicit tier list is the opposite: the
    # caller has named exactly what it wants. A floor under a stated intent is
    # not a safety net — it is the function quietly not doing what it was asked.
    # See the comment in ci-select.sh for the full distinction.
    DUMPSTAR_CI_SELECTED=""
    for _tier in ${DUMPSTAR_CI_TIERS}; do
        case " ${DUMPSTAR_CI_ALL_TIERS} " in
            *" ${_tier} "*) DUMPSTAR_CI_SELECTED="${DUMPSTAR_CI_SELECTED}${_tier} " ;;
            *)
                echo "ERROR: DUMPSTAR_CI_TIERS contains '${_tier}', which is not one of: ${DUMPSTAR_CI_ALL_TIERS}" >&2
                exit 1
                ;;
        esac
    done
    SELECTION_SOURCE="explicit tier list (DUMPSTAR_CI_TIERS): ${DUMPSTAR_CI_TIERS}"
elif [ -n "${DUMPSTAR_CI_CHANGED_FILES:-}" ]; then
    # Named but unreadable is a refusal, not a fallback. A caller that meant to
    # select and instead typo'd a path would otherwise get a silent full run,
    # which is the safe direction but hides a broken workflow indefinitely.
    [ -f "$DUMPSTAR_CI_CHANGED_FILES" ] || {
        echo "ERROR: DUMPSTAR_CI_CHANGED_FILES=${DUMPSTAR_CI_CHANGED_FILES} is not a file." >&2
        exit 1
    }
    DUMPSTAR_CI_SELECTED="$(dumpstar_ci_select_tiers < "$DUMPSTAR_CI_CHANGED_FILES" | tr '\n' ' ')"
    # `|| true` because an EMPTY list must reach the selector, which reads it as
    # "we do not know what changed" and runs everything. Without it grep's exit 1
    # on a zero count took the whole script down under `set -e` — a full-suite
    # answer turned into a dead run, found by tests/ci/select_test.sh §4.
    SELECTION_SOURCE="$(grep -c '' < "$DUMPSTAR_CI_CHANGED_FILES" || true) changed path(s) in ${DUMPSTAR_CI_CHANGED_FILES}"
else
    DUMPSTAR_CI_SELECTED="$(dumpstar_ci_all_tiers | tr '\n' ' ')"
    SELECTION_SOURCE="no changed-path list given — running everything"
fi

# Guard for one tier's steps. Exit status 2 from the library means the name is
# not a tier at all — a typo, which must be fatal here rather than reading as
# "not selected" and skipping a gate on every run from now on.
# `-q` asks the same question without announcing a skip — for a caller that has
# to ask about several tiers to answer one question (does anything below need
# the image built?) and would otherwise print a skip line per tier it asks about.
tier() {
    local rc=0 quiet=""
    if [ "$1" = "-q" ]; then
        quiet=yes
        shift
    fi
    dumpstar_ci_tier_selected "$1" || rc=$?
    case "$rc" in
        0) return 0 ;;
        2)
            echo "ERROR: ci.sh guards on '$1', which is not one of: ${DUMPSTAR_CI_ALL_TIERS}" >&2
            exit 1
            ;;
    esac
    if [ -z "$quiet" ]; then
        echo ""
        echo "==> (skipped: tier '$1' is not required by these changes)"
    fi
    return 1
}

step "CI tiers"
echo "    selection: ${SELECTION_SOURCE}"
for _t in $DUMPSTAR_CI_ALL_TIERS; do
    if dumpstar_ci_tier_selected "$_t"; then echo "    RUN   $_t"; else echo "    skip  $_t"; fi
done

if [ -n "${DUMPSTAR_CI_SELECT_ONLY:-}" ]; then
    echo ""
    echo "==> DUMPSTAR_CI_SELECT_ONLY set — plan printed, nothing run."
    exit 0
fi

# --- 0a. Are the tools this script calls actually here? -----------------------
#
# cargo, node, podman and sqlite3 are called below and installed by neither this
# script nor the repository. That held for as long as CI only ever ran on one
# box where somebody had installed them by hand — the workflow carried a step
# that prepended ~/.cargo/bin and ~/.local/bin to PATH, added because the very
# first run of that workflow died with `cargo: command not found`, and that PATH
# fixup was the only place the dependency was ever written down. On a per-run VM
# there is no such box, and this names what is missing before any gate runs
# rather than three gates in, on a machine that is about to be destroyed.
#
# AFTER the SELECT_ONLY exit, deliberately. That mode promises to print the plan
# and touch nothing, and tests/ci/select_test.sh §4 asks for the plan on
# whatever box it runs on — a dependency check ahead of it turns "what would you
# run" into "I cannot run", which is a different question.
#
# It checks; it does not install. Installing needs sudo, and a test script that
# quietly apt-installs on someone's laptop is worse than the gap it closes.
# THE PER-USER TOOLCHAINS GO ON PATH HERE, NOT IN runner-deps.sh.
#
# rustup installs cargo under ~/.cargo/bin and several tools land in
# ~/.local/bin, and a runner service is not a login shell, so nothing has
# sourced the profile that adds either. runner-deps.sh does export them — but
# only into ITSELF, and a child process cannot export to its parent. So
# runner-deps.sh found cargo, reported every dependency present, and `cargo fmt
# --check` two hundred lines later died with status 127.
#
# The old workflow did this with `echo "$HOME/.cargo/bin" >> $GITHUB_PATH`,
# which worked and was reachable only from CI. Doing it here means a developer,
# a polecat and CI all get the same answer from `command -v cargo`, and there is
# no second copy of the list to drift.
#
# $HOME-relative, never a literal /home/<user>: a hardcoded path would put one
# box's layout back into the repo, which is the thing pd-rf7c took out.
case ":${PATH}:" in
    *":${HOME}/.cargo/bin:"*) ;;
    *) PATH="${HOME}/.cargo/bin:${PATH}" ;;
esac
case ":${PATH}:" in
    *":${HOME}/.local/bin:"*) ;;
    *) PATH="${HOME}/.local/bin:${PATH}" ;;
esac
export PATH

echo "==> Runner dependencies"
bash "$SCRIPT_DIR/runner-deps.sh" --check

# --- 0b. Container store + disk floor ----------------------------------------

# shellcheck source=deploy/store-lib.sh
. "$SCRIPT_DIR/store-lib.sh"
# Unset means "not answered here, ask the host" — ~/.config/dumpstar/store.env.
# Set, including set to empty, means the caller decided and is left alone.
# Answered nowhere means Podman's default store, which is also prod's.
dumpstar_store_load_config
dumpstar_store_activate
# A store created before pd-3zjt keeps sharing prod's rootless-netns scaffolding
# whatever the generated config says — podman pins that at store creation. Asked
# once per run, because otherwise nothing on the box ever says the one-time
# teardown is still outstanding. Warns; never fails the run. See store-lib.sh.
dumpstar_store_split_check

step "Prune cargo incremental artifacts"
# Cargo incremental state grows unboundedly across gate runs and is expendable:
# it speeds up re-compiles but is never needed for correctness, and the rlibs
# in deps/ survive — so only the changed crates recompile on the next run.
# When disk falls inside the warning band (< 2×floor) it can exhaust the floor
# the next check enforces (db-6gob). Remove it first in that case, giving the
# floor check a reading for what actual build work needs rather than for what
# previous runs left behind.
_ci_inc_target="${CARGO_TARGET_DIR:-${REPO_DIR}/target}"
if [ -d "$_ci_inc_target" ]; then
    _ci_inc_free="$(df -BG --output=avail "$_ci_inc_target" | tail -n1 | tr -dc '0-9')"
    _ci_inc_warn="$(( ${DUMPSTAR_DISK_FLOOR_GB:-10} * 2 ))"
    if [ "${_ci_inc_free:-99}" -lt "$_ci_inc_warn" ]; then
        echo "  ${_ci_inc_free}G free on $(df --output=target "$_ci_inc_target" | tail -n1) — below warn (${_ci_inc_warn}G), pruning incremental"
        while IFS= read -r -d '' _ci_inc_dir; do
            _ci_inc_kb="$(du -sk "$_ci_inc_dir" 2>/dev/null | cut -f1 || echo 0)"
            rm -rf -- "$_ci_inc_dir" 2>/dev/null || true
            echo "  -${_ci_inc_kb}K: ${_ci_inc_dir#"${_ci_inc_target}/"}"
        done < <(find "$_ci_inc_target" -maxdepth 2 -name incremental -type d -print0 2>/dev/null)
    else
        echo "  ${_ci_inc_free}G free — above warn (${_ci_inc_warn}G), skipping"
    fi
fi
unset _ci_inc_target _ci_inc_free _ci_inc_warn _ci_inc_dir _ci_inc_kb

step "Disk floor check"
# Before the build, not after it dies: at 697M free a cargo link failed with
# `ld terminated with signal 7 [Bus error]`, which reads as a toolchain bug and
# cost real time to diagnose (pd-fite).
#
# THE DISKS A RUN ACTUALLY WRITES TO, named ONCE and spent twice — here, and
# again before every parallel dispatch below. Two spellings of this list is how
# one of them quietly stops covering a disk, which is the shape of the bug this
# array replaces (pd-20ia).
#
#   $HOME              prod's default podman store and its volumes, plus the
#                      toolchain caches on a box that has not relocated them
#   DUMPSTAR_STORE_ROOT  the non-prod container store, where the host moved it
#   TMPDIR             every mktemp under deploy/ and tests/ — the gates' work
#                      directories, the source trees the isolation guards copy,
#                      and the per-gate output ci-parallel.sh buffers
#   CARGO_TARGET_DIR   where the compile writes, and the disk pd-fite's bus
#                      error actually happened on — a cargo LINK died there
#   CARGO_HOME         the registry cache and the downloaded crate sources
#
# The last three arms are not hypothetical, and each names a disk the first two
# cannot see. On the deployment box /tmp is its own LVM volume, so a full /tmp
# is invisible to $HOME: this check reported `/ has 40G free — ok` with 818M
# left on /tmp, which is below the very number that produced the bus error
# above. And .github/workflows/ci.yml sets CARGO_TARGET_DIR and CARGO_HOME onto
# /workspaces DELIBERATELY, so that the compile's largest writes stay off the
# volume production runs from — which means the shipped configuration puts the
# link that failed on a fourth device the floor named nowhere (pd-6jyd). The
# toolchain caches are relocated by symlink on this box for the same reason, so
# the parenthetical on the $HOME arm above does not cover them here; df follows
# a symlink, so naming "$HOME/.cargo" as a PATH measures wherever it points.
#
# diskcheck.sh reports each DEVICE once and walks up to the nearest existing
# ancestor of a path that does not exist yet, so on a box that has relocated
# nothing these arms cost exactly nothing — which is the argument the TMPDIR
# arm was added on.
DUMPSTAR_CI_DISK_PATHS=("$HOME" "${DUMPSTAR_STORE_ROOT:-$HOME}" "${TMPDIR:-/tmp}" \
	"${CARGO_TARGET_DIR:-${REPO_DIR}/target}" "${CARGO_HOME:-$HOME/.cargo}")
bash "$SCRIPT_DIR/diskcheck.sh" --floor "${DUMPSTAR_CI_DISK_PATHS[@]}"

# Once here is enough while one gate runs at a time and the previous one's
# teardown has already returned its space. Three at a time is three images,
# three volumes and three MinIO stores deep at once, so the same guard runs
# again before EVERY parallel dispatch — over the same list, copied rather than
# re-spelled (see the note above).
# shellcheck source=deploy/ci-parallel.sh
. "$SCRIPT_DIR/ci-parallel.sh"
DUMPSTAR_PAR_DISK_PATHS=("${DUMPSTAR_CI_DISK_PATHS[@]}")
dumpstar_par_reset

DF_BEFORE="$(df -h "$HOME" | tail -n1)"

# --- 1. Clean up any stale ci instance --------------------------------------

step "Cleaning up stale '${INSTANCE}' instance..."
bash "$SCRIPT_DIR/teardown.sh" "$INSTANCE" --purge 2>/dev/null || true
# `until=24h` matters for the same reason the instance name does: a bare
# `image prune -f` is global, so it will happily delete a dangling layer that
# a concurrently-running polecat's build is still using. Age-filtering keeps
# the housekeeping without reaching into another run's build.
podman image prune -f --filter "until=24h" >/dev/null 2>&1 || true

# Tear the ci instance down again on exit, whatever happens.
cleanup() {
    local rc=$?
    # Anything still running in the parallel wave goes first, and by SIGTERM, so
    # each gate's own EXIT trap gets to remove the containers it created. A
    # cancelled run would otherwise leave a MinIO and a Nessie behind per gate.
    dumpstar_par_kill_all
    bash "$SCRIPT_DIR/teardown.sh" "$INSTANCE" --purge 2>/dev/null || true
    # The one name this run added purely to hand the image to the gates. Every
    # tier tagged the same image under its own name, and untagging one name of a
    # multi-named image leaves the image and the other names alone — so this
    # cannot pull the image out from under a gate, and the next run's build
    # still finds the layers.
    podman rmi "$BUILD_IMAGE" 2>/dev/null || true
    # After the teardown noise, so "which step failed, and with what status" is
    # the last thing in the log rather than something to infer from where it
    # stops.
    # Asked before the line below, and asked whatever that line would have
    # said: the final step has no boundary after it, and a movement re-reads
    # everything printed above it — including a pass. The verdict overrides the
    # exit status in both directions. A green run that was tampered with must
    # not report green; a red one must not send the reader to look at their own
    # diff. And it REPLACES the "CI FAILED during step" line rather than
    # following it, because "failed" and "void" are different answers and
    # printing both in the same breath is how a reader keeps the wrong one.
    if ! dumpstar_treewatch_check "after the run finished"; then
        diag "!!   last step completed: ${CURRENT_STEP}"
        dumpstar_treewatch_verdict "$rc"
        exit "$DUMPSTAR_TREEWATCH_EXIT"
    fi
    [[ $rc -eq 0 ]] || diag "!! CI FAILED during step: ${CURRENT_STEP} (status ${rc})"
}
trap cleanup EXIT

# --- 1b. Lint tier ------------------------------------------------------------
# Hermetic and sub-second: proves the diagnostics above actually report a
# silenced failure, before spending ten minutes on gates that rely on them.
#
# THIS TIER ALWAYS RUNS — it is what a docs-only PR gets, and what keeps such a
# run capable of going red. Everything in it reads the tree and compiles
# nothing, which is what makes it affordable unconditionally.

if tier lint; then
    step "Harness diagnostics self-test (tests/lib/diagnostics_test.sh)"
    bash "$REPO_DIR/tests/lib/diagnostics_test.sh"

    # Same tier, same reason: a host port picked from a band instead of taken
    # from the kernel has been found and fixed in five files now, and each time
    # the fix reached one of them. §6-§8 of this gate assert on the tree, so a
    # sixth relapse fails here in a second rather than forty minutes in as
    # "address already in use". See tests/lib/ports.sh.
    # Same tier, and it guards the pager itself. Alarming's first real page was a
    # false one: every unit ships OnFailure=dumpstar-alert@%n, so each CI
    # disaster-recovery drill paged Ryan's phone when it tore itself down as
    # intended — nine times overnight on 2026-08-12/13 (pd-n0lf). The gate decides
    # who may page, and it has to be right in BOTH directions: the assertions that
    # matter most are the prod ones, because a gate that silences too much turns
    # into a backup that quietly stopped running and nobody hears about it.
    # Same tier, and it guards a gate that can SKIP the whole suite. The key is
    # (epoch, tree, tier set); if the tier set ever falls out of the key, a
    # docs-only pass silently certifies fourteen container gates that never ran.
    # §1 asserts exactly that, in both directions, along with the TTL, the epoch
    # and the refusal to key a dirty tree.
    step "Tree-hash cache (deploy/ci-cache.sh --self-test)"
    bash "$REPO_DIR/deploy/ci-cache.sh" --self-test

    # Same tier, same reason as the alert gate: this one decides whether Ryan's
    # phone rings at night. Its assumption ("a collection changes daily") was wrong
    # and paged him three times for a healthy backup on 2026-08-14. The cases that
    # matter are the ones proving the fix did not disarm the real alarm.
    step "Backup freshness (tests/alarming/backup_freshness_test.sh)"
    bash "$REPO_DIR/tests/alarming/backup_freshness_test.sh"

    # And the test that replaced it on the live tenant path. Same tier, same
    # reason, and one case in particular: a tenant the sidecar has never named
    # PASSED while the grace came from the tenant file's own timestamp (pd-30yy).
    # Every proxy tried for that grace failed toward SILENCE, so the matrix drives
    # the real function against a stubbed sidecar and varies the file's age
    # underneath it to prove the file decides nothing.
    step "Backup correspondence (tests/alarming/backup_correspondence_test.sh)"
    bash "$REPO_DIR/tests/alarming/backup_correspondence_test.sh"

    step "Alert gate (deploy/alert-gate.sh --self-test)"
    bash "$REPO_DIR/deploy/alert-gate.sh" --self-test

    step "Harness host-port self-test (tests/lib/ports_test.sh)"
    bash "$REPO_DIR/tests/lib/ports_test.sh"

    # Same tier, same shape of ratchet: the harnesses used to spend about two
    # minutes a run on fixed sleeps that asserted nothing except how fast this
    # box is. §1-§4 prove the replacement asks before it sleeps and — the one
    # that matters — that it GIVES UP, since an unbounded poll would hang CI
    # instead of failing it. §5-§6 assert on the tree, so a `sleep 8` cannot
    # come back one reasonable-looking line at a time. See tests/lib/wait.sh.
    step "Harness condition-polling self-test (tests/lib/wait_test.sh)"
    bash "$REPO_DIR/tests/lib/wait_test.sh"

    # Same tier, same shape of ratchet, and it guards what a container gate is
    # ALLOWED TO CONCLUDE. Three gates asked a real bucket what was in it with
    # `mc ls … 2>/dev/null`, which reports a listing that died and a store that
    # is empty identically. That cost a red run on 2026-08-26 (pd-cxq4) — and
    # the other direction is worse: "alice's prefix is empty after the drop"
    # and "nothing was replicated to the bucket root" are what a dead listing
    # also says, so a deletion could be reported PROVEN having looked at
    # nothing. §6 states the rule over the tree. See tests/lib/objects.sh.
    step "Harness object-listing self-test (tests/lib/objects_test.sh)"
    bash "$REPO_DIR/tests/lib/objects_test.sh"

    # Same tier, same class one layer down: what a container gate may conclude
    # when the container could not RESOLVE the peer it was launched to talk to.
    # Seventeen gates run two at a time, each churning containers on its own
    # user-defined network, and rootless podman serves every one of those names
    # from a single aardvark-dns that is reconfigured on every container
    # appearing or disappearing. A job that lands in that window gets EAI_AGAIN
    # — no answer, as distinct from the NXDOMAIN a name that is really gone
    # gives — and tests/lake/value_snapshots.sh reported it as "the 2026-08-09
    # build failed" on 2026-09-05 (run 33994136151), three minutes after its own
    # readiness probe had resolved the same name and 54 minutes after the same
    # commit went green. The retry is narrow on purpose: these gates ASSERT that
    # a partial run exits 2, so anything but the resolution signature is the
    # job's own answer and is returned unretried. See tests/lib/netrun.sh.
    step "A container that never resolved its peer (tests/lib/netrun_test.sh)"
    bash "$REPO_DIR/tests/lib/netrun_test.sh"

    # And the question those polls ask. `litestream ltx` prints a column header
    # for a replica that holds NOTHING and prints nothing at all when it cannot
    # reach S3, so `ltx … | grep -q .` reads an empty replica as full and an
    # unreachable bucket as empty. The second half is pd-reyy — a gate that
    # reported a replica gone and then restored it, healthy, three assertions
    # later, re-run by hand every time instead of read. §5-§6 are the ratchet:
    # pd-nt1k fixed this predicate in one harness of three, which is what a
    # per-file fix does.
    # Hermetic — recorded ltx output, no podman. See tests/lib/litestream.sh.
    step "Replica state has three answers, not two (tests/lib/litestream_test.sh)"
    bash "$REPO_DIR/tests/lib/litestream_test.sh"

    # Same tier, and it guards every gate below that builds the image: the
    # builder stage rode a moving tag, upstream retagged it from bookworm to
    # trixie, and the image started shipping a binary that cannot exec against
    # the bookworm runtime (pd-pejn). Reading the Containerfile costs a second;
    # finding out in step 4 costs the build first. See
    # tests/container/base_images_test.sh.
    step "Container base-image pins (tests/container/base_images_test.sh)"
    bash "$REPO_DIR/tests/container/base_images_test.sh"

    # Same rule, the OTHER image this repo runs. `litestream:latest` moved
    # 0.5.16 -> 0.5.17 under this box, 0.5.17 downgraded the one log message
    # deploy/backup-check.sh judges every tenant from to DEBUG, and the litestream,
    # drill and alarming gates went red together on a change that touched none of
    # them (pd-pfxf). Twenty-five minutes of container tier to learn that a tag
    # moved. This says it in under a second, and holds the log level the checker
    # depends on beside the version. See tests/litestream/image_pin_test.sh.
    step "Litestream image pin + log level (tests/litestream/image_pin_test.sh)"
    bash "$REPO_DIR/tests/litestream/image_pin_test.sh"

    # The S4 acceptance claims (non-first restore addressing, a re-created handle
    # with its control arm, a replica that stops advancing) run against the shipped
    # tree and against copies with one fault seeded in each; every fault must be
    # caught. Hermetic. See tests/litestream/seeded_fault_test.sh.
    step "Backup claims are seen to fail (tests/litestream/seeded_fault_test.sh)"
    bash "$REPO_DIR/tests/litestream/seeded_fault_test.sh"

    # Same tier, same shape of ratchet, one layer over from the pins: a gate
    # that builds an image at a per-checkout tag and never removes it. The tag
    # outlives the worktree that made it and nothing collects it, so the cost
    # shows up months later as a full disk (pd-5aba) rather than as a failing
    # gate. Hermetic and sub-second. See tests/lib/images_test.sh.
    step "Container gates remove their own images (tests/lib/images_test.sh)"
    bash "$REPO_DIR/tests/lib/images_test.sh"

    # Same tier, same shape again, and this one guards the browser tier's own
    # approval path. Narrowing `--update-snapshots` to one viewport was the
    # README's documented way to approve a subset; a rendering change reaches
    # both viewports, so what it actually produced was a stale baseline that
    # stayed green on the branch that made it and went red on the next one
    # (pd-4tce, from pd-tf4h). The gate refuses the pair, asserts every
    # scripted approval arrives through the guard, and checks both baseline
    # directories against routes.json. Hermetic and sub-second — it runs
    # Playwright not at all. See tests/visual/approval_test.sh.
    step "Baseline approvals cover every viewport (tests/visual/approval_test.sh)"
    bash "$REPO_DIR/tests/visual/approval_test.sh"

    # Path selection is itself a gate now, and one whose failure mode is a tier
    # quietly not running. Hermetic and sub-second, so it sits here rather than
    # behind the compile. See tests/ci/select_test.sh.
    step "CI tier selection fails closed (tests/ci/select_test.sh)"
    bash "$REPO_DIR/tests/ci/select_test.sh"

    # And the other half of the same story: the gates that no longer run where
    # they are written. Its failure mode is a gate that got queued nowhere and
    # runs never, which a green run cannot show you. Hermetic — the concurrency
    # is real but the gates are `true` and `sleep`. See tests/ci/parallel_test.sh.
    step "Parallel gate runner: cap, disk floor, no masked failure (tests/ci/parallel_test.sh)"
    bash "$REPO_DIR/tests/ci/parallel_test.sh"

    # And the guard over this script's own footing. A run whose checkout is
    # rebased underneath it reports on a tree that never existed — pd-vnbc,
    # where clippy produced seven errors naming symbols from no commit and the
    # rebase had already aborted by the time anyone looked. The case that makes
    # this gate worth its second is §3: because that rebase ABORTED, HEAD ended
    # where it began, so the obvious implementation of the guard passes it
    # green. Hermetic. See tests/ci/treewatch_test.sh.
    step "Worktree watch: a run whose tree moves is void (tests/ci/treewatch_test.sh)"
    bash "$REPO_DIR/tests/ci/treewatch_test.sh"

    # And the idiom that made the gate above lie. `cmd | grep -q PAT` under
    # `pipefail` reports the writer's SIGPIPE, so a SUCCESSFUL match takes the
    # failure branch — but only once the writer outgrows a pipe buffer, which is
    # why treewatch_test.sh passed on master and went red on a branch carrying
    # seven merges with no change of its own. Seventeen sites carried it. §1
    # demonstrates the inversion in a live shell rather than asserting a
    # document, so this cannot rot into folklore. Hermetic, sub-second.
    step "No pipeline into grep -q, which inverts under pipefail (tests/ci/grepq_test.sh)"
    bash "$REPO_DIR/tests/ci/grepq_test.sh"

    step "No committed sqlite under tests/ (tests/ci/fixture_untracked_test.sh)"
    bash "$REPO_DIR/tests/ci/fixture_untracked_test.sh"

    step "Nessie catalog backup: stale/corrupt alarm, restore-drill failures (tests/lake/nessie_backup_test.sh)"
    bash "$REPO_DIR/tests/lake/nessie_backup_test.sh"

    # The force_spill dispatch input: default off, and only true yields ec2-only.
    step "force_spill dispatch input shape (tests/ci/spill_input_test.sh)"
    bash "$REPO_DIR/tests/ci/spill_input_test.sh"

    # A failed systemctl start used to exit silently, destroying the evidence
    # along with the ephemeral runner. dump_unit_diagnostics captures status
    # and journal before exit; this asserts it is defined, contains both
    # commands, and is wired to every abort path (pd-rpgu).
    step "Unit startup evidence captured before exit (tests/ci/unit_diagnostics_test.sh)"
    bash "$REPO_DIR/tests/ci/unit_diagnostics_test.sh"

    # Same tier again, and the same shape of bug: a layer that fires and says
    # nothing. §6 of the alarming gate proves Layer 2 PUSHES; this proves what
    # it pushes is readable — the causal line first, no OCI metadata, no
    # systemd boilerplate — against journal tails captured from the real units
    # (pd-pwk8).
    step "Alert page content (tests/alarming/journal_summary_test.sh)"
    bash "$REPO_DIR/tests/alarming/journal_summary_test.sh"

    # And the third question about the same page, after "does it fire" and "can
    # it be read": should it be sent AT ALL. Four byte-identical value-snapshots
    # pages in four nights is how a channel gets swiped away unread, and the
    # outage that came next reached nobody (pd-hqdt). The assertions that matter
    # are the ones proving suppression did not disarm anything — a changed
    # signature, a second unit, an escalation, a stamp that cannot be trusted.
    step "Repeat pages are suppressed, changed ones are not (tests/alarming/alert_suppress_test.sh)"
    bash "$REPO_DIR/tests/alarming/alert_suppress_test.sh"

    # The standing decision the whole lake design rests on — no tenant data ever
    # enters it — existed only as comments in five files, so adding a tenant_id
    # column to an Iceberg schema or opening a tenant SQLite in a lake job broke
    # nothing (pd-cgi9). This is the missing mechanical half. It reads the tree
    # and stands up no MinIO and no Nessie, so it runs HERE rather than in the
    # two-minute lake tier it is named after — and a docs-only PR runs it too.
    # See tests/lake/tenant_isolation_test.sh.
    step "Tenant data stays out of the lake (tests/lake/tenant_isolation_test.sh)"
    bash "$REPO_DIR/tests/lake/tenant_isolation_test.sh"

    # And the half without which the one above is a guess. The inbound leg
    # (pd-8lw7) gave that guard four new sections about a zone that did not
    # exist when it was written, and a section whose corpus is empty passes
    # forever — three gates went green that way in one day on this repo. This
    # breaks the property on purpose, one violation at a time, and requires the
    # SPECIFIC assertion to be the one that reddens; the last block is the
    # opposite claim, that the tenant zone being legitimately tenant-keyed
    # fires nothing. Hermetic — it copies the source trees to a temp dir — but
    # it runs the guard once per case, so it is tens of seconds rather than
    # sub-second. See tests/lake/tenant_isolation_selftest.sh.
    step "That guard has been seen RED (tests/lake/tenant_isolation_selftest.sh)"
    bash "$REPO_DIR/tests/lake/tenant_isolation_selftest.sh"

    # ci.yml has no concurrency: block, provisions through ephemeral-ci@v2 with
    # the 60s spill, and tears down under if: always().
    step "Shared Rust compile cache wiring (tests/ci/rust_cache_test.sh)"
    bash "$REPO_DIR/tests/ci/rust_cache_test.sh"

    step "ci.yml shape (tests/ci/workflow_test.sh)"
    bash "$REPO_DIR/tests/ci/workflow_test.sh"

    step "deploy.yml shape (tests/ci/deploy_workflow_test.sh)"
    bash "$REPO_DIR/tests/ci/deploy_workflow_test.sh"

    # A formatter check, not a compile — `cargo fmt` parses and never builds,
    # so it costs a second and belongs with the lint tier rather than behind
    # `cargo test`.
    step "cargo fmt --check"
    ( cd "$REPO_DIR" && cargo fmt --check )
fi

# --- 2. Rust gates ----------------------------------------------------------

# Both Rust compiles in this run (here, and the image's release build below)
# read and write the shared sccache store, so unchanged crates are fetched.
# shellcheck source=deploy/rust-cache.sh
. "$SCRIPT_DIR/rust-cache.sh"
if tier -q rust || tier -q isolation || tier -q container; then
    dumpstar_rust_cache_enable
fi

# Cheapest and likeliest-to-fail first (sp-l2xovg): a one-word lint used to
# fail the run only after `cargo test` had spent ten minutes. clippy compiles
# every target, so it also rejects a compile error; --profile test makes it
# build what `cargo test` then REUSES (a plain clippy uses the dev profile and
# recompiles every crate for the lint alone). The frontend's type check follows,
# then the tests. `set -e` stops the run at the first failure.
if tier rust; then
    step "cargo clippy --all-targets"
    ( cd "$REPO_DIR" && cargo clippy --all-targets --profile test -- -D warnings )
    dumpstar_rust_cache_report "clippy"
fi

# --- 2. Frontend gate (before cargo test) -------------------------------------------------------

if tier frontend; then
    step "Frontend: npm ci && npm test && npm run check && npm run build"
    (
        cd "$REPO_DIR/frontend"
        npm ci
        # Design-token gates: WCAG AA contrast for every declared pairing, the
        # reference/semantic layer split, and the two ratchets — raw colour and
        # raw dimension — which fail on any INCREASE in values chosen outside
        # the token layer. Node's built-in runner, no extra deps.
        npm test
        npm run check
        npm run build
    )
fi

# --- 2. Rust tests ---
if tier rust; then
    step "cargo test"
    ( cd "$REPO_DIR" && cargo test )
    dumpstar_rust_cache_report "cargo test"
fi

# --- 2a. Cross-tenant isolation gate (sp-c3fq3p) ------------------------------
# Its own tier so a red names the property that broke, and so a caller can ask
# for exactly it (DUMPSTAR_CI_TIERS=isolation): tenant B's valid JWT cannot read
# or delete tenant A's rows, a leftover tenant header grants nothing, a route
# registered without the access layer fails the ratchet, and every /api response
# is `private, no-store`. In-process against the real router and a throwaway
# data dir: no production instance, no network. The tests live in
# `tests::isolation` in crates/dumpstar-server/src/lib.rs; the tier is
# selected by that module path, so a test added there is in the gate.
if tier isolation; then
    step "Cross-tenant isolation gate (cargo test -p dumpstar-server tests::isolation)"
    # A filter that matches nothing passes. Refuse that: renaming the module
    # must break this step, not empty it.
    _iso_n="$(cd "$REPO_DIR" && cargo test -p dumpstar-server --lib -- tests::isolation:: --list 2>/dev/null | grep -c ': test$' || true)"
    if [ "${_iso_n:-0}" -lt 8 ]; then
        echo "ERROR: the isolation gate selects ${_iso_n:-0} tests (expected at least 8); tests::isolation was renamed or emptied." >&2
        exit 1
    fi
    ( cd "$REPO_DIR" && cargo test -p dumpstar-server --lib -- tests::isolation:: )
fi

# --- 2b. Deploy-script gates ------------------------------------------------
# The low-disk guard, the store-root resolution and the unit-file install are
# shell, so they get a shell test — including one that shows the guard actually
# firing, and one that drives deploy.sh over stale installed units and asserts
# every one comes back matching the shipped template (pd-2t6u).

if tier deploy; then
    step "Deploy scripts: unit install + store resolution + low-disk guard + scheduling"
    bash "$REPO_DIR/tests/deploy/run.sh"

    # The one store gate that is not hermetic, and it has to be: that a non-prod
    # store's rootless-netns scaffolding is its OWN is a fact about podman 4.9.3
    # honouring `[engine] tmp_dir`, not about this repo, and the shell test can
    # only assert that the file asking for it was written. Sharing that directory
    # is what wedged prod's network namespace nightly (pd-3zjt). No image, no
    # container, no network — one `podman unshare --rootless-netns true` in a
    # throwaway store, about a second, and it removes nothing outside it.
    step "Container store: the rootless-netns split, against real podman"
    bash "$REPO_DIR/tests/store/netns_split.sh"

    # The second non-hermetic store gate, and non-hermetic for the same reason:
    # that a build orphans its stage images, that `-f dangling=true` does not
    # list the layer cache, and that removing the previous generation therefore
    # frees bytes WITHOUT costing the next build its cache are all facts about
    # podman. Nothing on this box collected those layers, so every build left
    # ~2 GB behind on the filesystem prod runs from — 5.1 GB found in prod's
    # store — until / hit 91% and this script stopped at its own disk floor with
    # the whole container tier below it unrunnable (pd-h3wy). ~17s, pulls
    # nothing (its fixture is FROM scratch), and works only inside a throwaway
    # store it tears down.
    step "Container store: a build collects what the build before it orphaned"
    bash "$REPO_DIR/tests/store/orphans.sh"

    # The third non-hermetic build gate, and non-hermetic for the third time for
    # the same reason: that podman's COPY preserves the context's mtimes and that
    # cargo then calls an old source fresh against a newer artifact are facts
    # about podman and cargo. The cargo target cache mount outlives the build and
    # its `id=` names the checkout PATH — but a CI runner is one path holding
    # every tree over time, so a build was handed another tree's rlibs, compiled
    # nothing, and shipped that tree's binaries (pd-wjmd). tests/deploy/run.sh
    # §11c holds the shell half; this is the two real builds. Its fixture is a
    # hello-world crate on the builder base the real Containerfile names, so it
    # pulls nothing the container tier does not already need, and its cache id is
    # unique to the run and removed on the way out.
    step "Container store: the image ships the binaries its own sources build"
    bash "$REPO_DIR/tests/store/cargo_cache.sh"
fi

# --- 3b. The image, once ----------------------------------------------------
# The container gate needs the shipped image, and so will every gate armed
# after it; building once here and exporting the name keeps that to one
# invocation of the builder per run.
#
# Podman's layer cache made four of them cheap rather than free — measured on
# the CI box: ~4s for a repeat over identical content, against 5m23s when the
# sources changed and 7m27s with the layer cache dropped. Nothing GUARANTEES the
# cheap case: a `podman image prune`, a store teardown or a cold box turns the
# four repeats back into four full compiles, and that regression reads as "CI is
# slow again" rather than as a bug. Building here and exporting the name makes it
# structural. Standalone runs of any of those gates still build their own image;
# see deploy/image-lib.sh for the contract.
#
# It is built only if one of those tiers is actually selected. A build nobody
# needs is exactly the cost pd-s2mj removed, and a docs-only PR must not pay it
# just because the build moved up here. `tier` prints a skip line per call, so
# the question is asked through the raw predicate and answered once.

# Through `tier -q`, so a typo in this list is the same fatal error it is
# anywhere else rather than reading as "not selected" and quietly never
# building. No `break`: every name is put to the library.
NEEDS_IMAGE=no
for _t in container; do
    if tier -q "$_t"; then NEEDS_IMAGE=yes; fi
done

if [ "$NEEDS_IMAGE" = yes ]; then
    step "Building the container image (once — every gate below tags this one)"
    # Through the same helper the gates use, so a caller who already has the
    # image (DUMPSTAR_PREBUILT_IMAGE set in the environment) skips even this build.
    # shellcheck source=deploy/image-lib.sh
    . "$SCRIPT_DIR/image-lib.sh"
    dumpstar_image_ensure "$BUILD_IMAGE" "$REPO_DIR"
    export DUMPSTAR_PREBUILT_IMAGE="$BUILD_IMAGE"
    echo "    ${BUILD_IMAGE} ($(podman image inspect --format '{{.Id}}' "$BUILD_IMAGE" | cut -c1-12))"
else
    echo ""
    echo "==> (skipped: no selected tier needs the container image)"
fi

# --- 4. Container gate ------------------------------------------------------
# Selected whenever the browser tier is — the screenshots are taken against
# this instance, and the SvelteKit build they are screenshotting is baked into
# its image. ci-select.sh refuses a selection that has one without the other.

PORT=""
if tier container; then
    # The production unit, against real podman and systemd: it starts, serves
    # /health on loopback only, and the serving check fails when the container is
    # killed (design row S5). The script builds its own throwaway instance through
    # setup.sh --test, and configures Access against a test JWKS server because the
    # server refuses to start without one; the plain "start it and curl /" step this
    # replaces could not pass for that reason. PORT stays empty: no gate below this
    # point is armed yet to need a running instance (section 5).
    step "Production unit: starts, loopback-only, serving check (tests/deploy/prod_unit.sh)"
    bash "$REPO_DIR/tests/deploy/prod_unit.sh" "$INSTANCE"
fi

# --- 5. Gates not armed yet --------------------------------------------------
# The backup, tenant-isolation, schema-version, lakehouse, refresh and browser
# gates this script was copied with each drive a Pokémon catalog fixture, a
# pokemontcg.io upstream or a tenant layout that dumpstar does not have yet.
# They are not run here, and they are not run half-armed: a gate whose reds are
# its own fixture's is worse than no gate (it trains everyone to re-run). Each is
# re-armed by the dumpstar bead that gives it something real to assert —
# tenancy (the isolation gate), the backup drill, the catalog seam (the lakehouse
# and refresh gates), the SWU UI fixture (the browser tier). Their scripts stay
# under tests/ so that bead starts from a working harness rather than a blank
# page, and deploy/ci-select.sh is the single list of what runs.

# --- 16. Run the queued gates -----------------------------------------------
# Everything queued above, two at a time, with the disk floor checked before
# every dispatch and each gate's whole output printed under its own name as it
# finishes. Nothing here decides WHICH gates run — that was settled by the tier
# guards above, so a skipped tier queues nothing and this step simply has less
# to do. See deploy/ci-parallel.sh.
#
# CURRENT_STEP carries the failed gates' names into the EXIT trap's diagnostic
# line, so a run that dies here still ends with "which gate, and with what
# status" as its last line rather than teardown noise.

step "Running the queued container gates in parallel"
PAR_RC=0
dumpstar_par_run || PAR_RC=$?
if [ "$PAR_RC" -ne 0 ]; then
    CURRENT_STEP="parallel gates: ${DUMPSTAR_PAR_FAILED:-see above}"
    exit "$PAR_RC"
fi

# The intents UI harness is intentionally not run here — see the header.
echo ""
echo "    (intents UI harness not run — see the note in this script's header)"

# --- Done -------------------------------------------------------------------

ELAPSED=$(( $(date +%s) - START_TIME ))
echo ""
# The whole point of the alternate store: a CI run must not eat the disk prod
# runs from. Printed every run so a regression shows up as a number, not as a
# mystery bus error three weeks later (pd-fite).
echo "==> Disk holding \$HOME (prod's):"
echo "    before: ${DF_BEFORE}"
echo "    after:  $(df -h "$HOME" | tail -n1)"
echo ""
echo "==> CI passed in ${ELAPSED}s."
