#!/usr/bin/env bash
# Vendor dumpster-kit-ui at the revision pinned in kit.lock.
#   scripts/kit-sync.sh <dumpster-kit checkout>
# The checkout must be AT the pinned revision; the files are copied byte for
# byte and kit.lock's sha256 table is rewritten. tests/kit-pin.test.js fails
# when anything under src/lib/kit drifts from that table, so the kit stays the
# one implementation (edit it there, bump the pin here).
set -euo pipefail
kit=${1:?usage: kit-sync.sh <dumpster-kit checkout> [revision]}
here=$(cd "$(dirname "$0")/.." && pwd)
rev=${2:-$(sed -n 's/^revision *= *//p' "$here/kit.lock")}
head=$(git -C "$kit" rev-parse HEAD)
[ "$head" = "$rev" ] || { echo "kit checkout is at $head, pin is $rev" >&2; exit 1; }
src=$kit/crates/dumpster-kit-ui/assets
dst=$here/src/lib/kit
rm -rf "$dst"
mkdir -p "$dst/ui"
cp "$src"/components/*.svelte "$src/components/index.ts" "$dst/ui/"
cp "$src/tokens.css" "$src/tokens.ts" "$src/contrast-pairs.json" "$dst/"
{
	echo "revision = $rev"
	(cd "$dst" && find . -type f | LC_ALL=C sort | xargs sha256sum)
} > "$here/kit.lock"
