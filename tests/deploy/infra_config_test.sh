#!/usr/bin/env bash
# tier: T2
# covers: deploy/infra-config.sh
# tests/deploy/infra_config_test.sh — infra-config.sh against a canned `terraform output -json`
# and stubbed aws and podman (sp-16o41o).
#
#   1. lake.env gets DUMPSTAR_TENANT_AWS_PROFILE=dumpstar-tenant, distinct from AWS_PROFILE, and a
#      value already set is left alone.
#   2. aws/config carries the dumpstar-tenant profile, also when the file predates it.
#   3. --secrets writes one bootstrap secret holding [dumpstar] and [dumpstar-tenant], each from its
#      own Secrets Manager secret; a bootstrap secret without the tenant section is recreated.
#
# Hermetic: bash, jq.
#
#   bash tests/deploy/infra_config_test.sh
set -euo pipefail
REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0; fail=0
ok() { echo "  PASS  $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL  $1"; fail=$((fail + 1)); }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (want '$2', got '$3')"; fi; }

BIN="$WORK/bin"; HOME_DIR="$WORK/home"; SECRETS="$WORK/secrets"
mkdir -p "$BIN" "$HOME_DIR/.config/dumpstar/prod" "$SECRETS"
: >"$HOME_DIR/.config/dumpstar/prod/litestream.env"

cat >"$WORK/tf.json" <<'JSON'
{"region":{"value":"us-west-2"},"lake_bucket":{"value":"lake-b"},"backup_bucket":{"value":"bk-b"},
 "lake_secret_name":{"value":"dumpstar/lake"},"backup_secret_name":{"value":"dumpstar/backup"},
 "tenant_secret_name":{"value":"dumpstar/tenant"}}
JSON

cat >"$BIN/aws" <<'EOS'
#!/usr/bin/env bash
id=""
while [ $# -gt 0 ]; do [ "$1" = "--secret-id" ] && id="$2"; shift; done
n="${id#dumpstar/}"
printf '{"AWS_ACCESS_KEY_ID":"AKIA-%s","AWS_SECRET_ACCESS_KEY":"SK-%s"}\n' "$n" "$n"
EOS
cat >"$BIN/podman" <<'EOS'
#!/usr/bin/env bash
# podman secret {inspect [--showsecret] N | create N - | rm N}
[ "$1" = secret ] || exit 1
shift; verb="$1"; shift
[ "${1:-}" = "--showsecret" ] && shift
case "$verb" in
inspect) [ -f "$STORE/$1" ] && cat "$STORE/$1" ;;
create) cat >"$STORE/$1" ;;
rm) rm -f "$STORE/$1" ;;
esac
EOS
chmod +x "$BIN/aws" "$BIN/podman"

run() { HOME="$HOME_DIR" STORE="$SECRETS" PATH="$BIN:$PATH" DUMPSTAR_TF_OUTPUT="$WORK/tf.json" \
    bash "$REPO_DIR/deploy/infra-config.sh" prod "$@" >/dev/null; }
LAKE_ENV="$HOME_DIR/.config/dumpstar/lake.env"
CFG="$HOME_DIR/.config/dumpstar/prod/aws/config"

run
check "tenant profile written to lake.env" dumpstar-tenant "$(grep '^DUMPSTAR_TENANT_AWS_PROFILE=' "$LAKE_ENV" | cut -d= -f2)"
if [ "$(grep '^AWS_PROFILE=' "$LAKE_ENV" | cut -d= -f2)" != dumpstar-tenant ]; then ok "tenant profile is not the catalog profile"; else bad "tenant profile equals AWS_PROFILE"; fi
grep -q '^\[profile dumpstar-tenant\]' "$CFG" && ok "aws/config has the tenant profile" || bad "aws/config lacks the tenant profile"

sed -i 's/^DUMPSTAR_TENANT_AWS_PROFILE=.*/DUMPSTAR_TENANT_AWS_PROFILE=custom/' "$LAKE_ENV"
run
check "a value already set is left alone" custom "$(grep '^DUMPSTAR_TENANT_AWS_PROFILE=' "$LAKE_ENV" | cut -d= -f2)"

printf '[profile dumpstar]\nregion = x\n' >"$CFG"
run
grep -q '^\[profile dumpstar-tenant\]' "$CFG" && ok "an aws/config older than the tenant profile gains it" || bad "old aws/config not upgraded"

printf '[dumpstar]\naws_access_key_id=old\naws_secret_access_key=old\n' >"$SECRETS/dumpstar-prod-s3-bootstrap"
run --secrets
B="$SECRETS/dumpstar-prod-s3-bootstrap"
check "stale bootstrap secret is recreated with the tenant section" 1 "$(grep -c '^\[dumpstar-tenant\]' "$B")"
check "lake section carries the lake key" 1 "$(grep -c '^aws_access_key_id=AKIA-lake$' "$B")"
check "tenant section carries the tenant key" 1 "$(grep -c '^aws_access_key_id=AKIA-tenant$' "$B")"
check "backup secret holds only the backup profile" "[dumpstar-backup]" "$(head -n1 "$SECRETS/dumpstar-prod-backup-s3")"
cp "$B" "$WORK/before"
run --secrets
cmp -s "$B" "$WORK/before" && ok "a complete bootstrap secret is left alone" || bad "complete secret was rewritten"

echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
