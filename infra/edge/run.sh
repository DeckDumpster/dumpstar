#!/usr/bin/env bash
# One instance's edge, in its own workspace: bash run.sh <prod|env-1|env-2|env-3> <plan|apply|import-plan> [terraform args]
# Needs backend.tfvars (bucket, assume_role = { role_arn = ... }; see backend.tfvars.example) and common.tfvars (see common.auto.tfvars.example),
# CLOUDFLARE_API_TOKEN and POCKETID_API_TOKEN in the environment. Never prints an output value
# that is a secret: there are none.
set -euo pipefail
inst="${1:?usage: run.sh <prod|env-1|env-2|env-3> <plan|apply|import-plan>}"
verb="${2:?plan|apply|import-plan}"
shift 2
cd "$(dirname "$0")"
[ -f "$inst.tfvars" ] || { echo "run.sh: no such instance: $inst" >&2; exit 1; }
# Providers are downloaded once into a persistent cache, not into every working copy.
export TF_PLUGIN_CACHE_DIR="${TF_PLUGIN_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/terraform/plugin-cache}"
mkdir -p "$TF_PLUGIN_CACHE_DIR"
terraform init -input=false -reconfigure -backend-config=backend.tfvars >/dev/null
terraform workspace select -or-create "$inst" >/dev/null
args=(-var-file=common.tfvars -var-file="$inst.tfvars")
case "$verb" in
plan) terraform plan "${args[@]}" "$@" ;;
apply) terraform apply "${args[@]}" "$@" ;;
import-plan) terraform plan "${args[@]}" -var-file=prod.import.tfvars "$@" ;;
*) echo "run.sh: unknown verb $verb" >&2; exit 1 ;;
esac
