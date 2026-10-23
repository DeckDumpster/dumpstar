#!/usr/bin/env bash
# tier: T0
# covers: infra/*
# tests/ci/tf_backend_test.sh — the S3 backend takes its role as assume_role = { role_arn = ... }.
# Hermetic: reads text only, so it runs where terraform is not installed.
# The syntax of each example is a plain key/block form that terraform reads as -backend-config.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
fail=0
bad() { echo "FAIL: $*" >&2; fail=1; }

# No instruction passes role_arn at the top level of the backend config.
hits="$(grep -rnE 'backend-config="?role_arn=|^[[:space:]]*role_arn[[:space:]]*=' \
  "$root/infra" --include='*.md' --include='*.sh' --include='*.example' \
  | grep -v 'deploy_role_arn' || true)"
# Lines inside an assume_role block are indented and follow "assume_role"; examples are checked below.
hits="$(grep -E 'backend-config="?role_arn=|\(bucket, role_arn' <<<"$hits" || true)"
[ -z "$hits" ] || bad "old-form backend instruction:"$'\n'"$hits"

for root_dir in infra infra/edge infra/edge-shared infra/env; do
  ex="$root/$root_dir/backend.tfvars.example"
  [ -f "$ex" ] || { bad "$root_dir: no backend.tfvars.example"; continue; }
  grep -qE '^bucket[[:space:]]*=[[:space:]]*"' "$ex" || bad "$ex: no bucket"
  # role_arn must sit inside an assume_role = { } block, never at the top level.
  awk -v f="$ex" '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    /^assume_role[[:space:]]*=[[:space:]]*\{/ { inb=1; next }
    inb && /^\}/ { inb=0; next }
    /role_arn/ && !inb { print f ": top-level role_arn"; bad=1 }
    inb && /role_arn[[:space:]]*=[[:space:]]*"arn:/ { seen=1 }
    END { if (!seen) { print f ": no assume_role { role_arn }"; bad=1 } exit bad }' "$ex" >&2 || fail=1
done

# The runner that reads backend.tfvars points at the example.
grep -q 'backend.tfvars.example' "$root/infra/edge/run.sh" <<<"$(cat "$root/infra/edge/run.sh")" \
  || bad "infra/edge/run.sh does not name backend.tfvars.example"

[ "$fail" = 0 ] && echo "tf_backend_test: ok"
exit "$fail"
