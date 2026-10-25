#!/usr/bin/env bash
# tier: T0
# covers: .github/workflows/ci.yml
# tests/ci/workflow_test.sh — the shape of .github/workflows/ci.yml (design row S1).
#
# What it holds, each a thing that went wrong somewhere else once:
#
#   * NO `concurrency:` BLOCK, anywhere in the file. GitHub keeps one PENDING run
#     per group and evicts the rest, so several PRs pushed together do not queue
#     — they are cancelled before the VM spill can act (deckdumpster sp-r4zvel).
#   * the VM comes from DeckDumpster/ephemeral-ci/provision@v2 with
#     `spill-after-seconds: '60'`, and goes away through teardown@v2 under
#     `if: always()` — a cancelled or failed run otherwise leaves the VM running.
#   * every provision call passes `spill-market: ${{ vars.SPILL_MARKET }}` (the org spot
#     switch; a composite action cannot read vars) unless a `# spill-market pinned:` comment says why.
#   * the job the merge queue reads is named `gate`.
#   * ci.yml, deploy.yml and data-jobs.yml are the only workflows; deploy.yml's own shape is held
#     by tests/ci/deploy_workflow_test.sh.
#
# §1 is the SEEN-RED arm: each rule is broken in a scratch copy and the checker
# must refuse it. §2 runs the checker over the real file. Hermetic, sub-second.

set -euo pipefail
cd "$(dirname "$0")/../.."

# problems <workflow-file> -> prints one line per broken rule
problems() {
    local wf="$1" body
    has() { [ "$(grep -c -E "$1" "$wf")" -gt 0 ]; }
    if has '^[[:space:]]*concurrency:'; then echo "has a concurrency: block"; fi
    has 'DeckDumpster/ephemeral-ci/provision@v2' || echo "does not use provision@v2"
    has 'DeckDumpster/ephemeral-ci/teardown@v2' || echo "does not use teardown@v2"
    has "spill-after-seconds: '60'" || echo "spill-after-seconds is not '60'"
    has '^ +spill: +ec2-only$' || echo "spill: is not ec2-only (every test run goes to EC2, Ryan 2026-10-15)"
    # the teardown job must be conditional on always()
    body="$(awk '/^  teardown:/{f=1;next} f&&/^  [a-z]/{f=0} f' "$wf")"
    [ "$(grep -c -E '^ +if: always\(\)' <<<"$body")" -gt 0 ] \
        || echo "teardown does not run under if: always()"
    has '^  gate:' || echo "no job named gate"
    # every provision call follows the org spot switch, or pins a market with a comment saying why
    if [ "$(grep -c -E 'ephemeral-ci/provision@' "$wf")" -gt "$(grep -c -E '^ +spill-market: +\$\{\{ *vars\.SPILL_MARKET *\}\}' "$wf")" ] \
        && ! grep -q -E '^ +# spill-market pinned:' "$wf"; then
        echo "a provision call lacks spill-market: \${{ vars.SPILL_MARKET }}"
    fi
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
wf=.github/workflows/ci.yml

echo "==> §1 each rule is refused when broken"

{ echo 'concurrency:'; echo '  group: ci'; cat "$wf"; } >"$scratch/top.yml"
expect "a repo-wide concurrency: block" "concurrency" "$scratch/top.yml"

sed -E 's/^(  gate:)/  gate:\n    concurrency:\n      group: g/' "$wf" >"$scratch/job.yml"
expect "a job-level concurrency: block" "concurrency" "$scratch/job.yml"

sed "s/spill-after-seconds: '60'/spill-after-seconds: '600'/" "$wf" >"$scratch/spill.yml"
expect "a slower spill" "spill-after-seconds" "$scratch/spill.yml"

sed -E 's/^( +)if: always\(\)$/\1if: success()/' "$wf" >"$scratch/teardown.yml"
expect "a teardown that skips failed runs" "if: always()" "$scratch/teardown.yml"

sed 's|DeckDumpster/ephemeral-ci/provision@v2|DeckDumpster/ephemeral-ci/provision@v1|' "$wf" >"$scratch/pin.yml"
expect "an unpinned provision" "provision@v2" "$scratch/pin.yml"

grep -v 'spill-market:' "$wf" >"$scratch/market.yml"
expect "a provision call without spill-market" "spill-market" "$scratch/market.yml"

echo "==> §2 the real workflow"
out="$(problems "$wf")"
if [ -z "$out" ]; then echo "    ok   ci.yml holds every rule"; else echo "    FAIL ci.yml: $out"; fail=1; fi

others="$(find .github/workflows -type f ! -name ci.yml ! -name deploy.yml ! -name data-jobs.yml | sort)"
if [ -z "$others" ]; then
    echo "    ok   ci.yml, deploy.yml and data-jobs.yml are the only workflows"
else
    echo "    FAIL other workflows exist: $others"
    fail=1
fi

[ "$fail" = 0 ] && echo "workflow: all assertions passed" || { echo "workflow: FAILED"; exit 1; }
