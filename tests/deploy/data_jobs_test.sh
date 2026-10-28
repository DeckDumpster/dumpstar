#!/usr/bin/env bash
# tier: T1
# covers: deploy/data-jobs.sh deploy/data-jobs-lib.sh
# tests/deploy/data_jobs_test.sh — deploy.sh runs the data jobs a change affects
# (sp-g20aoh). Hermetic: a scratch git repo and a stub systemctl.
#   §1 a commit touching crates/dumpstar-swu lands, derives, fetches pictures, then prices, once, and logs both
#   §2 a commit touching only frontend runs neither
#   §3 no recorded sha, or a failed job, runs / is retried; the sha is recorded only on success
#   §4 the dispatch workflow runs data-jobs.sh on the prod runner
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
scratch="$(mktemp -d)"; trap 'rm -rf "$scratch"' EXIT
export HOME="$scratch/home" XDG_STATE_HOME="$scratch/state"; mkdir -p "$HOME"
mkdir -p "$scratch/bin"
cat >"$scratch/bin/systemctl" <<'STUB'
#!/usr/bin/env bash
[ "$1" = --user ] && shift
case "$1" in
    cat) exit 0 ;;
    start) shift; echo "$*" >>"$STUB_LOG"; [ -z "${STUB_FAIL:-}" ] ;;
esac
STUB
chmod +x "$scratch/bin/systemctl"; export PATH="$scratch/bin:$PATH" STUB_LOG="$scratch/log"
fail=0
check() { if eval "$2"; then echo "    ok   $1"; else echo "    FAIL $1"; fail=1; fi; }

. "$REPO/deploy/data-jobs-lib.sh"
# the lake check is stubbed: STUB_NO_LAKE=1 means this instance has no lakehouse
dumpstar_lake_installed() { [ -z "${STUB_NO_LAKE:-}" ]; }
r="$scratch/repo"; git init -q "$r"; git -C "$r" config user.email t@t; git -C "$r" config user.name t
commit() { mkdir -p "$r/$(dirname "$1")"; echo "$RANDOM" >"$r/$1"; git -C "$r" add -A; git -C "$r" commit -qm "$1"; }
# deploy_step mirrors the block at the foot of deploy.sh
deploy_step() {
    : >"$STUB_LOG"
    if reason="$(dumpstar_data_jobs_needed "$r" t)"; then
        echo "ran: $reason"; dumpstar_data_jobs_run t || return 1
    else echo "skipped: $reason"; fi
    dumpstar_record_deployed_sha "$r" t
}
check "deploy.sh wires the same block" "grep -q 'dumpstar_data_jobs_needed' '$REPO/deploy/deploy.sh' && grep -q 'dumpstar_data_jobs_run' '$REPO/deploy/deploy.sh'"

echo "==> §1 catalog change runs both, once"
commit README.md; deploy_step >/dev/null
commit crates/dumpstar-swu/src/crosswalk.rs
deploy_step >"$scratch/out"
check "land, derive, pictures, then prices" "[ \"\$(paste -sd' ' '$STUB_LOG')\" = '--wait dumpstar-refresh@t.service --wait dumpstar-derive@t.service --wait dumpstar-catalog-sync@t.service --wait dumpstar-prices@t.service' ]"
check "all logged" "grep -q 'refresh@t.service: done' '$scratch/out' && grep -q 'derive@t.service: done' '$scratch/out' && grep -q 'catalog-sync@t.service: done' '$scratch/out' && grep -q 'prices@t.service: done' '$scratch/out'"
deploy_step >/dev/null
check "redeploying the same sha runs nothing" "[ ! -s '$STUB_LOG' ]"

echo "==> §2 frontend-only runs neither"
commit frontend/src/app.html; deploy_step >/dev/null
check "no job started" "[ ! -s '$STUB_LOG' ]"
commit data/remedial/x.json; deploy_step >/dev/null
check "remedial data runs them all" "[ \$(wc -l <'$STUB_LOG') -eq 4 ]"

echo "==> §3 first deploy and failure"
rm -rf "$XDG_STATE_HOME"; deploy_step >/dev/null
check "no recorded sha runs" "[ \$(wc -l <'$STUB_LOG') -eq 4 ]"
commit crates/dumpstar-swu/src/a.rs
STUB_FAIL=1 deploy_step >/dev/null 2>&1 && bad=1 || bad=0
check "a failed job fails the step" "[ $bad = 0 ]"
deploy_step >/dev/null
check "the failed run is retried" "[ \$(wc -l <'$STUB_LOG') -eq 4 ]"

echo "==> §5 no lakehouse: the catalog jobs run, the lake prices job is skipped"
: >"$STUB_LOG"; STUB_NO_LAKE=1 dumpstar_data_jobs_run t >"$scratch/out5"
check "only the catalog jobs started" "[ \"\$(paste -sd' ' '$STUB_LOG')\" = '--wait dumpstar-refresh@t.service --wait dumpstar-derive@t.service --wait dumpstar-catalog-sync@t.service' ]"
check "the skip is said" "grep -q 'prices@t.service: no lakehouse installed' '$scratch/out5'"

echo "==> §4 dispatch workflow"
wf="$REPO/.github/workflows/data-jobs.yml"
check "dispatch only, on the prod runner, runs data-jobs.sh prod" \
    "grep -q '^  workflow_dispatch:' '$wf' && grep -q dumpstar-prod '$wf' && grep -q 'deploy/data-jobs.sh\" prod' '$wf' && ! grep -q '^  push:' '$wf'"
[ "$fail" = 0 ] && echo "data jobs: all assertions passed" || { echo "data jobs: FAILED"; exit 1; }
