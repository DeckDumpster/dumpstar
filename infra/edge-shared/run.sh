#!/usr/bin/env bash
# The shared Access group, one state: bash run.sh <plan|apply|output> [terraform args]
# Needs backend.tfvars (see backend.tfvars.example) and common.tfvars (see common.auto.tfvars.example),
# and CLOUDFLARE_API_TOKEN in the environment. Apply this before any infra/edge instance.
set -euo pipefail
verb="${1:?usage: run.sh <plan|apply|output>}"
shift
cd "$(dirname "$0")"
terraform init -input=false -reconfigure -backend-config=backend.tfvars >/dev/null
case "$verb" in
plan) terraform plan -var-file=common.tfvars "$@" ;;
apply) terraform apply -var-file=common.tfvars "$@" ;;
output) terraform output "$@" ;;
*) echo "run.sh: unknown verb $verb" >&2; exit 1 ;;
esac
