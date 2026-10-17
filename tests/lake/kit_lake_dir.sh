#!/usr/bin/env bash
# tier: T1
# covers: tests/lake/kit_lake_dir.sh Cargo.toml
# Print the source directory of a kit crate (default: the lake crate) at the revision the
# workspace pins. The lake layout (zones, key builders, tenant.rs) is the kit's;
# the isolation guards read it from there, so a bump of the pin is what they
# re-prove. Fails closed: no output and a non-zero status if it cannot be found.
set -euo pipefail
crate="${1:-dumpster-kit-lake}"
cd "$(dirname "$0")/../.."
dir="$(cargo metadata --format-version 1 2>/dev/null |
	jq -er --arg crate "$crate" '[.packages[] | select(.name == $crate) | .manifest_path | sub("/Cargo.toml$"; "")] | first')" ||
	{
		echo "${crate} is not in the workspace graph" >&2
		exit 1
	}
printf '%s\n' "$dir"
