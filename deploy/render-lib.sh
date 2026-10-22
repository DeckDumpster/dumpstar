#!/usr/bin/env bash
#
# One rendering of an instance's Quadlets and env files (sp-qgjpvj).
#
# deploy/dumpstar.container, deploy/dumpstar-litestream.container and deploy/env/*.env are
# TEMPLATES. An instance is those templates plus six declared parameters, and nothing else:
#
#   INSTANCE    the name                       dumpstar-<name>.service, image tag, config dir
#   HOSTNAME    public hostname                the /mcp resource, the OIDC redirect URL
#   PORT        published loopback port        PublishPort=127.0.0.1:<port>:8080
#   VOLUME      the data volume                Volume=<volume>:/data
#   SECRETS_DIR where the env files and secrets live (paths only; nothing is inlined)
#   S3_PREFIX   the backup prefix              LITESTREAM_S3_PATH, LITESTREAM_S3_REGISTRY_PATH
#
# tests/deploy/render_test.sh renders prod against the checked-in golden files and prod against
# another instance, and fails on any difference those six do not account for. Both
# deploy/units-lib.sh (what deploy does to a host) and deploy/render.sh (what the test and the
# env tool read) go through dumpstar_render_sed_args, so they cannot disagree.
#
# Sourced, not executed.

DUMPSTAR_PROD_HOSTNAME="${DUMPSTAR_PROD_HOSTNAME:-star.dumpster.cards}"

# dumpstar_render_sed_args <instance> <hostname> <port> <volume> <secrets-dir> <s3-prefix>
# Prints, one per line, the sed -e arguments that fill every placeholder. An empty hostname
# deletes the lines that only make sense on a public hostname (the /mcp resource).
dumpstar_render_sed_args() {
    local instance="$1" hostname="$2" port="$3" volume="$4" secrets="$5" s3="$6"
    printf '%s\n' -e "s|{{INSTANCE}}|${instance}|g" \
        -e "s|{{HOSTNAME}}|${hostname}|g" \
        -e "s|{{PORT}}|${port}|g" \
        -e "s|{{VOLUME}}|${volume}|g" \
        -e "s|{{SECRETS_DIR}}|${secrets}|g" \
        -e "s|{{S3_PREFIX}}|${s3}|g"
    [ -n "$hostname" ] || printf '%s\n' -e '/^Environment=DUMPSTAR_MCP_RESOURCE=/d'
}

# dumpstar_render_instance <outdir> <instance> <hostname> <port> <volume> <secrets-dir> <s3-prefix> [checkout]
# Writes the two Quadlets and the four env files, with the repo's own templates. <checkout> is
# the deploy checkout's absolute path as the host sees it (the seventh parameter; it is baked
# into the sidecar's litestream.yml mount); it defaults to the checkout rendering.
dumpstar_render_instance() {
    local out="$1" instance="$2" repo_dir args=() checkout="${8:-}"
    repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    [ -n "$checkout" ] || checkout="$repo_dir"
    shift
    mapfile -t args < <(dumpstar_render_sed_args "$@")
    mkdir -p "$out"
    local alert=()
    dumpstar_units_alerting "$instance" || alert=(-e '/^OnFailure=/d')
    sed "${args[@]}" "${alert[@]}" "$repo_dir/deploy/dumpstar.container" >"$out/dumpstar-${instance}.container"
    sed "${args[@]}" "${alert[@]}" -e "s|{{REPO_DIR}}|${checkout}|g" \
        "$repo_dir/deploy/dumpstar-litestream.container" >"$out/dumpstar-litestream-${instance}.container"
    local f
    for f in access oidc metrics litestream; do
        sed "${args[@]}" "$repo_dir/deploy/env/${f}.env" >"$out/${f}.env"
    done
}
