#!/usr/bin/env bash
#
# Render an instance's Quadlets and env files into a directory, touching nothing else.
#
#   bash deploy/render.sh <outdir> <name> [--hostname H] [--port N] [--volume V]
#                         [--secrets DIR] [--s3-prefix P]
#
# Defaults: hostname star.dumpster.cards for prod and empty otherwise; port from
# deploy/prod-origin-port for prod; volume dumpstar-<name>-data; secrets %h/.config/dumpstar/<name>;
# S3 prefix <name>. See deploy/render-lib.sh.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/alert-gate.sh
. "$SCRIPT_DIR/alert-gate.sh"
# shellcheck source=deploy/render-lib.sh
. "$SCRIPT_DIR/render-lib.sh"

[ $# -ge 2 ] || { sed -n '3,10p' "$0" >&2; exit 2; }
OUT="$1"; NAME="$2"; shift 2
HOST=""; [ "$NAME" = prod ] && HOST="$DUMPSTAR_PROD_HOSTNAME"
PORT=""; [ "$NAME" = prod ] && PORT="$(tr -d '[:space:]' <"$SCRIPT_DIR/prod-origin-port")"
VOLUME="dumpstar-${NAME}-data"; SECRETS="%h/.config/dumpstar/${NAME}"; S3="$NAME"
while [ $# -gt 0 ]; do
    case "$1" in
        --hostname) HOST="$2" ;;
        --port) PORT="$2" ;;
        --volume) VOLUME="$2" ;;
        --secrets) SECRETS="$2" ;;
        --s3-prefix) S3="$2" ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift 2
done
[ -n "$PORT" ] || { echo "ERROR: --port is required for ${NAME}" >&2; exit 2; }
dumpstar_render_instance "$OUT" "$NAME" "$HOST" "$PORT" "$VOLUME" "$SECRETS" "$S3"
