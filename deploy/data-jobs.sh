#!/usr/bin/env bash
# Run catalog-apply then the price build on an instance now, rather than at the
# next timer (sp-g20aoh). The on-demand half of deploy.sh's post-deploy jobs;
# .github/workflows/data-jobs.yml runs it on the prod runner.
#
#   bash deploy/data-jobs.sh <instance>
set -euo pipefail
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
[ $# -eq 1 ] || { echo "Usage: bash deploy/data-jobs.sh <instance>" >&2; exit 1; }
# shellcheck source=deploy/data-jobs-lib.sh
. "$(cd "$(dirname "$0")" && pwd)/data-jobs-lib.sh"
dumpstar_data_jobs_run "$1"
