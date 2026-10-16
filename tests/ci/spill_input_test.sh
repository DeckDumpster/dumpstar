#!/usr/bin/env bash
# tier: T0
# covers: .github/workflows/ci.yml
# tests/ci/spill_input_test.sh — the force_spill dispatch input on ci.yml.
# Exists, boolean, defaults false; provision's `spill:` is ec2-only always —
# every test run goes to EC2 (Ryan 2026-10-15). Hermetic.
set -euo pipefail
cd "$(dirname "$0")/../.."
wf=.github/workflows/ci.yml
fail=0
check() { if ! "$@"; then echo "FAIL: $*" >&2; fail=1; fi; }

in_block="$(awk '/^  workflow_dispatch:/{f=1;next} f&&/^  [a-z]/{f=0} f' "$wf")"
check grep -q '^      force_spill:' <<<"$in_block"
check grep -q 'type: boolean' <<<"$in_block"
check grep -q 'default: false' <<<"$in_block"

spill="$(grep -E '^ +spill: ' "$wf")"
check test "$(grep -c . <<<"$spill")" = 1
expr="$(sed -E 's/^ +spill: +//' <<<"$spill")"
check test "$expr" = "ec2-only"
[ "$fail" = 0 ] && echo "ok: force_spill shape"
exit "$fail"
