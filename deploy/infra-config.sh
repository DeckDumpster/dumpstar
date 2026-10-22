#!/usr/bin/env bash
#
# Writes this host's lake and backup config from the Terraform outputs in infra/ (sp-zcne5d).
#
# Usage:
#   bash deploy/infra-config.sh <instance> [--secrets]
#
# Writes (0600, never over a value already set):
#   ~/.config/dumpstar/lake.env                  DUMPSTAR_LAKE_S3_BUCKET/_REGION, AWS_PROFILE=dumpstar,
#                                                DUMPSTAR_TENANT_AWS_PROFILE=dumpstar-tenant
#   ~/.config/dumpstar/<instance>/litestream.env  LITESTREAM_S3_BUCKET/_REGION, AWS_PROFILE=dumpstar-backup
#   ~/.config/dumpstar/<instance>/aws/config      the three profiles (non-secret; gates the sidecar)
#
# --secrets also creates, when absent, the podman secrets that carry the credentials, read
# from Secrets Manager (`aws secretsmanager`, your own identity) straight into `podman secret
# create`, so a key never reaches a file or a terminal:
#   dumpstar-<instance>-s3-bootstrap   lake credential [dumpstar] and tenant-zone credential
#                                      [dumpstar-tenant] in one file (refresh, derive, ship, erase);
#                                      an existing secret without [dumpstar-tenant] is recreated
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
set_default "$LAKE_ENV" DUMPSTAR_TENANT_AWS_PROFILE dumpstar-tenant

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
if ! grep -q '^\[profile dumpstar-tenant\]' "${LS_DIR}/aws/config"; then
    printf '\n[profile dumpstar-tenant]\nregion = %s\n' "$REGION" >>"${LS_DIR}/aws/config"
    echo "    ${LS_DIR}/aws/config: profile dumpstar-tenant added"
fi

if [ "$WITH_SECRETS" = 1 ]; then
    secret_profile() { # <profile> <Secrets Manager name>: one credentials-file section
        aws secretsmanager get-secret-value --region "$REGION" --secret-id "$2" --query SecretString --output text |
            jq -r --arg p "$1" '"[\($p)]\naws_access_key_id=\(.AWS_ACCESS_KEY_ID)\naws_secret_access_key=\(.AWS_SECRET_ACCESS_KEY)"'
    }
    BOOTSTRAP="dumpstar-${INSTANCE}-s3-bootstrap"
    if podman secret inspect --showsecret "$BOOTSTRAP" 2>/dev/null | grep -q '\[dumpstar-tenant\]'; then
        echo "    podman secret ${BOOTSTRAP} exists, left alone"
    else
        if podman secret inspect "$BOOTSTRAP" >/dev/null 2>&1; then
            podman secret rm "$BOOTSTRAP" >/dev/null
            echo "    podman secret ${BOOTSTRAP} lacked [dumpstar-tenant], recreating"
        fi
        { secret_profile dumpstar "$(out lake_secret_name)"; echo; secret_profile dumpstar-tenant "$(out tenant_secret_name)"; } |
            podman secret create "$BOOTSTRAP" - >/dev/null
        echo "    podman secret ${BOOTSTRAP} created"
    fi
    BACKUP="dumpstar-${INSTANCE}-backup-s3"
    if podman secret inspect "$BACKUP" >/dev/null 2>&1; then
        echo "    podman secret ${BACKUP} exists, left alone"
    else
        secret_profile dumpstar-backup "$(out backup_secret_name)" | podman secret create "$BACKUP" - >/dev/null
        echo "    podman secret ${BACKUP} created"
    fi
fi
