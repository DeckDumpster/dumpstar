#!/usr/bin/env bash
#
# Writes this host's lake and backup config from the Terraform outputs in infra/ (sp-zcne5d).
#
# Usage:
#   bash deploy/infra-config.sh <instance> [--secrets]
#
# Writes (0600, never over a value already set):
#   ~/.config/dumpstar/lake.env                  DUMPSTAR_LAKE_S3_BUCKET/_REGION, AWS_PROFILE=dumpstar
#   ~/.config/dumpstar/<instance>/litestream.env  LITESTREAM_S3_BUCKET/_REGION, AWS_PROFILE=dumpstar-backup
#   ~/.config/dumpstar/<instance>/aws/config      the two profiles (non-secret; gates the sidecar)
#
# --secrets also creates, when absent, the podman secrets that carry the two credentials, read
# from Secrets Manager (`aws secretsmanager`, your own identity) straight into `podman secret
# create`, so a key never reaches a file or a terminal:
#   dumpstar-<instance>-s3-bootstrap   lake credential   (refresh, derive, ship, erase)
#   dumpstar-<instance>-backup-s3      backup credential (litestream, backup-check, restore)
#
# DUMPSTAR_TF_OUTPUT names a file of `terraform output -json` (test seam); otherwise it is run
# in infra/.
set -euo pipefail

INSTANCE="${1:?usage: infra-config.sh <instance> [--secrets]}"
WITH_SECRETS=0
[ "${2:-}" = "--secrets" ] && WITH_SECRETS=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONF="${HOME}/.config/dumpstar"
LAKE_ENV="${DUMPSTAR_LAKE_ENV:-${CONF}/lake.env}"
LS_DIR="${CONF}/${INSTANCE}"
LS_ENV="${LS_DIR}/litestream.env"

if [ -n "${DUMPSTAR_TF_OUTPUT:-}" ]; then
    OUT="$(cat "$DUMPSTAR_TF_OUTPUT")"
else
    OUT="$(terraform -chdir="${SCRIPT_DIR}/../infra" output -json)"
fi
out() { printf '%s' "$OUT" | jq -er ".$1.value"; }
REGION="$(out region)"
LAKE_BUCKET="$(out lake_bucket)"
BACKUP_BUCKET="$(out backup_bucket)"

# set_default <file> <key> <value>: assign the key unless the file already gives it a real
# value. A scaffolded `#KEY=…` or `KEY=CHANGE_ME` counts as unset; anything else is kept.
set_default() {
    local file="$1" key="$2" value="$3" current
    current="$(grep -E "^${key}=" "$file" | tail -n1 | cut -d= -f2- || true)"
    if [ -n "$current" ] && [ "$current" != "CHANGE_ME" ]; then
        echo "    ${file}: ${key} already set, left alone"
        return
    fi
    if grep -qE "^${key}=" "$file"; then
        sed -i "s|^${key}=.*|${key}=${value}|" "$file"
    else
        printf '%s=%s\n' "$key" "$value" >>"$file"
    fi
    echo "    ${file}: ${key} set"
}

mkdir -p "$CONF" "${LS_DIR}/aws"
chmod 700 "$CONF" "$LS_DIR" "${LS_DIR}/aws"

[ -f "$LAKE_ENV" ] || : >"$LAKE_ENV"
chmod 600 "$LAKE_ENV"
set_default "$LAKE_ENV" DUMPSTAR_LAKE_S3_BUCKET "$LAKE_BUCKET"
set_default "$LAKE_ENV" DUMPSTAR_LAKE_S3_REGION "$REGION"
set_default "$LAKE_ENV" AWS_PROFILE dumpstar

if [ ! -f "$LS_ENV" ]; then
    echo "infra-config: ${LS_ENV} does not exist — run deploy/setup.sh ${INSTANCE} first." >&2
    exit 1
fi
chmod 600 "$LS_ENV"
set_default "$LS_ENV" LITESTREAM_S3_BUCKET "$BACKUP_BUCKET"
set_default "$LS_ENV" LITESTREAM_S3_REGION "$REGION"
sed -i 's|^AWS_PROFILE=dumpstar$|AWS_PROFILE=dumpstar-backup|' "$LS_ENV"

if [ ! -f "${LS_DIR}/aws/config" ]; then
    printf '[profile dumpstar]\nregion = %s\n\n[profile dumpstar-backup]\nregion = %s\n' "$REGION" "$REGION" >"${LS_DIR}/aws/config"
    chmod 600 "${LS_DIR}/aws/config"
    echo "    ${LS_DIR}/aws/config written"
fi

if [ "$WITH_SECRETS" = 1 ]; then
    make_secret() { # <podman secret> <profile> <Secrets Manager name>
        if podman secret inspect "$1" >/dev/null 2>&1; then
            echo "    podman secret ${1} exists, left alone"
            return
        fi
        aws secretsmanager get-secret-value --region "$REGION" --secret-id "$3" --query SecretString --output text |
            jq -r --arg p "$2" '"[\($p)]\naws_access_key_id=\(.AWS_ACCESS_KEY_ID)\naws_secret_access_key=\(.AWS_SECRET_ACCESS_KEY)"' |
            podman secret create "$1" - >/dev/null
        echo "    podman secret ${1} created"
    }
    make_secret "dumpstar-${INSTANCE}-s3-bootstrap" dumpstar "$(out lake_secret_name)"
    make_secret "dumpstar-${INSTANCE}-backup-s3" dumpstar-backup "$(out backup_secret_name)"
fi
