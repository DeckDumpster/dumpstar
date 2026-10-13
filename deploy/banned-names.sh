#!/usr/bin/env bash
#
# The source site's identity and the removed Pokémon modules must not come back
# (design row S1, dumpstar 1).
#
# Dumpstar began as a copy of the Pokémon site's workspace. Two things were taken
# out of the copy and are checked here, over the tracked files, from the repo
# root:
#
#   1. THE IDENTITY — the old environment prefix, the old crate/unit prefix and
#      the old application name, in any tracked path or any tracked text.
#   2. THE POKÉMON MODULES — the variant model, the pokemontcg and Japan
#      catalogs, overrides, symbols, bundles, set discovery, standalone promos
#      and the sub-type map: as a file by its name, and as a Rust module
#      declaration.
#
# It reads `git ls-files`, so an untracked scratch file cannot fail a run and a
# tracked one cannot hide. Exit 0 is clean; exit 1 prints every offence as
# `path: why`. tests/ci/banned_names_test.sh proves each arm can fail.
#
# The words below are assembled from pieces so this file does not contain what
# it forbids. Fixtures that are verbatim captures of an upstream (fixtures/swu)
# are exempt: they are data, not names we chose.

set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

old_env="PK""DUMP"
old_prefix="pk""dump"
old_app="poke""dumpster"
identity="${old_env}|${old_prefix}|${old_app}"

# File names that were removed with the Pokémon modules.
module_files='(^|/)(japan|japan_series|overrides|symbols|bundles|set_discovery|standalone_promos|sub_type_map|variants|variant|pokemontcg|pokemon_tcg_data)\.(rs|json)$'
data_files='(^|/)(tcgcsv_sub_type_variants|variant_augmentations|tcgcsv_set_bridges|tcgcsv_set_discovery|standalone_promos|upstream_card_corrections)\.json$'
# `pub mod <removed>;` / `mod <removed>;` in Rust.
module_decl='^[[:space:]]*(pub(\([a-z]+\))? )?mod (japan|overrides|symbols|bundles|set_discovery|standalone_promos|sub_type_map|variants|variant|pokemontcg|pokemon_tcg_data);'

offences=0
report() { echo "$1: $2"; offences=$((offences + 1)); }

mapfile -t files < <(git ls-files | grep -v '^fixtures/swu/' || true)

for f in "${files[@]}"; do
    if [[ "${f,,}" =~ ${identity,,} ]]; then
        report "$f" "path carries the old identity '${BASH_REMATCH[0]}'"
    fi
    if [[ "$f" =~ $module_files ]]; then
        report "$f" "a removed Pokémon module file"
    elif [[ "$f" =~ $data_files ]]; then
        report "$f" "a removed Pokémon data file"
    fi
done

# Content. -I skips binary files (the committed SQLite fixtures are checked by
# the harness that regenerates them).
if [ "${#files[@]}" -gt 0 ]; then
    while IFS=: read -r path line name; do
        [ -n "$path" ] && report "$path:$line" "text carries the old identity '$name'"
    done < <(git grep -I -n -i -o -E "$identity" -- "${files[@]}" || true)

    while IFS=: read -r path line _; do
        [ -n "$path" ] && report "$path:$line" "declares a removed Pokémon module"
    done < <(git grep -I -n -E "$module_decl" -- "${files[@]}" || true)
fi

if [ "$offences" -gt 0 ]; then
    echo "banned-names: ${offences} offence(s). Dumpstar carries none of the source site's identity and none of the removed Pokémon modules." >&2
    exit 1
fi
echo "banned-names: clean (${#files[@]} tracked files checked)"
