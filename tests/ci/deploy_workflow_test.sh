#!/usr/bin/env bash
# tests/ci/deploy_workflow_test.sh — the shape of .github/workflows/deploy.yml
# (design row S5).
#
# What it holds:
#   * it triggers on the main-branch event and nothing else: no pull_request (a PR
#     must never reach prod), no manual or scheduled trigger.
#   * it runs on the dumpstar-prod runner, in the production checkout; never from the
#     runner workspace (a merge does not deploy from /workspaces-style checkouts).
#   * it runs `deploy.sh prod`, and ends by asking the origin whether it serves.
#
# §1 is the SEEN-RED arm: each rule is broken in a scratch copy and the checker must
# refuse it. §2 runs the checker over the real file. Hermetic, sub-second.

set -euo pipefail
cd "$(dirname "$0")/../.."

problems() {
    local wf="$1"
    has() { [ "$(grep -c -E "$1" "$wf")" -gt 0 ]; }
    has '^  push:' || echo "does not trigger on the main-branch event"
    has 'branches: \[main\]$' || echo "is not limited to main"
    if has '^  (pull_request|pull_request_target|workflow_dispatch|schedule):'; then
        echo "has a trigger other than the main-branch event"
    fi
    has 'dumpstar-prod' || echo "does not run on the dumpstar-prod runner"
    has 'deploy/deploy\.sh" prod' || echo "does not run deploy.sh prod"
    has 'PROD_CHECKOUT' || echo "does not deploy from the production checkout"
    if has 'github\.workspace|GITHUB_WORKSPACE|actions/checkout'; then
        echo "deploys from the runner workspace"
    fi
    has 'deploy/heartbeat\.sh" prod' || echo "does not check the origin after deploying"
    return 0
}

fail=0
expect() { # expect <description> <substring-of-problem> <file>
    local out
    out="$(problems "$3")"
    if [ "$(grep -c -F "$2" <<<"$out")" -gt 0 ]; then
        echo "    ok   $1"
    else
        echo "    FAIL $1 (checker said: ${out:-nothing})"
        fail=1
    fi
}

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
wf=.github/workflows/deploy.yml

echo "==> §1 each rule is refused when broken"
sed -E 's/^  push:/  pull_request:/' "$wf" >"$scratch/a.yml"
expect "a deploy triggered by pull requests" "trigger" "$scratch/a.yml"
sed -E 's/branches: \[main\]/branches: [main, dev]/' "$wf" >"$scratch/b.yml"
expect "a deploy not limited to main" "limited to main" "$scratch/b.yml"
{ cat "$wf"; printf '  workflow_dispatch:\n'; } >"$scratch/c.yml"
expect "a manual trigger" "other than" "$scratch/c.yml"
sed -E 's/^(    steps:)/\1\n      - uses: actions\/checkout@v4/' "$wf" >"$scratch/d.yml"
expect "a deploy from the runner workspace" "workspace" "$scratch/d.yml"
grep -v 'heartbeat\.sh' "$wf" >"$scratch/e.yml"
expect "a deploy that never asks the origin" "origin" "$scratch/e.yml"
sed 's/dumpstar-prod/ubuntu-latest/' "$wf" >"$scratch/f.yml"
expect "a deploy on a hosted runner" "dumpstar-prod" "$scratch/f.yml"

echo "==> §2 the real workflow"
out="$(problems "$wf")"
if [ -z "$out" ]; then echo "    ok   deploy.yml holds every rule"; else echo "    FAIL deploy.yml: $out"; fail=1; fi

[ "$fail" = 0 ] && echo "deploy workflow: all assertions passed" || { echo "deploy workflow: FAILED"; exit 1; }
