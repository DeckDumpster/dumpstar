#!/usr/bin/env bash
# Print the source directory of a kit crate (default: the lake crate) at the revision the
# workspace pins. The lake layout (zones, key builders, tenant.rs) is the kit's;
# the isolation guards read it from there, so a bump of the pin is what they
# re-prove. Fails closed: no output and a non-zero status if it cannot be found.
set -euo pipefail
crate="${1:-dumpster-kit-lake}"
cd "$(dirname "$0")/../.."
cargo metadata --format-version 1 --locked 2>/dev/null |
	python3 -c '
import json, os, sys
crate = sys.argv[1]
for p in json.load(sys.stdin)["packages"]:
    if p["name"] == crate:
        print(os.path.dirname(p["manifest_path"]))
        raise SystemExit
raise SystemExit(crate + " is not in the workspace graph")
' "$crate"
