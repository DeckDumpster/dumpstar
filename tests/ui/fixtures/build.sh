#!/usr/bin/env bash
# Build tests/ui/fixtures/{shared,collection}.sqlite from the text sources.
#
# The databases are never committed (two branches that touch the schema or
# the seeder would each regenerate a binary and git cannot merge them). They
# are produced by `dumpstar seed-fixture`, which is deterministic, and cached
# by the hash of everything that feeds it, so an unchanged tree builds once.
#
#   tests/ui/fixtures/build.sh [OUT_DIR]     (default: this directory)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
out="${1:-$here}"
cache_root="${DUMPSTAR_FIXTURE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/dumpstar-fixtures}"

# Inputs: the seeder, the catalog crate (schema.sql, formats.json, ...), the db
# crate (overlay schemas, shipped seeds), the lockfile and this script.
key="$(
    cd "$root"
    {
        find crates/dumpstar-cli/src crates/dumpstar-swu/src crates/dumpstar-db/src \
            -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum
        sha256sum Cargo.lock tests/ui/fixtures/build.sh
    } | sha256sum | cut -c1-16
)"

hit="$cache_root/$key"
if [ ! -f "$hit/shared.sqlite" ] || [ ! -f "$hit/collection.sqlite" ]; then
    tmp="$(mktemp -d "$cache_root.tmp.XXXXXX" 2>/dev/null || mktemp -d)"
    (cd "$root" && cargo run --quiet -p dumpstar-cli --bin dumpstar -- seed-fixture --out "$tmp")
    mkdir -p "$cache_root"
    rm -rf "$hit"
    mv "$tmp" "$hit"
fi

mkdir -p "$out"
cp "$hit/shared.sqlite" "$hit/collection.sqlite" "$out/"
echo "fixture $key -> $out"
