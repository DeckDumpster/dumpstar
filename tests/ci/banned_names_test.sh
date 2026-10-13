#!/usr/bin/env bash
# tests/ci/banned_names_test.sh — the source site's identity and the removed
# Pokémon modules must not come back (design row S1).
#
# deploy/banned-names.sh is the gate; this proves it can FAIL. A grep gate that
# has only ever been green is a gate nobody knows works, so §1 builds a throwaway
# git repository, plants each kind of offence in it and requires the gate to name
# it, and §2 requires the gate to pass on a tree without them. §3 runs it over
# the real tree.
#
# Hermetic, no compilation, sub-second: it runs in the lint tier.

set -euo pipefail
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="$REPO_DIR/deploy/banned-names.sh"

fail=0
check() { # check <description> <expected> <actual>
    if [ "$2" = "$3" ]; then
        echo "    ok   $1"
    else
        echo "    FAIL $1 (expected '$2', got '$3')"
        fail=1
    fi
}

# The words are assembled so this file does not contain what it forbids.
OLD_PREFIX="pk""dump"
OLD_ENV="PK""DUMP"
OLD_APP="poke""dumpster"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

mkrepo() { # mkrepo <dir>
    mkdir -p "$1"
    git -C "$1" init -q
    git -C "$1" config user.email t@example.com
    git -C "$1" config user.name t
    mkdir -p "$1/crates/dumpstar-ingest/src" "$1/data"
    echo 'pub mod tcgcsv;' >"$1/crates/dumpstar-ingest/src/lib.rs"
}
run_gate() { # run_gate <dir> -> prints exit status, leaves output in $scratch/out
    local rc=0
    (cd "$1" && git add -A && bash "$GATE" >"$scratch/out" 2>&1) || rc=$?
    echo "$rc"
}

echo "==> §1 each offence is refused, and named"

r="$scratch/env"; mkrepo "$r"
echo "export ${OLD_ENV}_HOME=/tmp" >"$r/run.sh"
check "an old environment prefix fails the gate" 1 "$(run_gate "$r")"
check "…and the gate names the file" 1 "$(grep -c 'run.sh' "$scratch/out")"

r="$scratch/crate"; mkrepo "$r"
mkdir -p "$r/crates/${OLD_PREFIX}-core"
echo x >"$r/crates/${OLD_PREFIX}-core/Cargo.toml"
check "an old crate or path name fails the gate" 1 "$(run_gate "$r")"

r="$scratch/app"; mkrepo "$r"
echo "see ${OLD_APP}" >"$r/notes.md"
check "the old app name fails the gate" 1 "$(run_gate "$r")"

r="$scratch/module"; mkrepo "$r"
echo '// removed' >"$r/crates/dumpstar-ingest/src/japan.rs"
check "a removed Pokémon module file fails the gate" 1 "$(run_gate "$r")"
check "…and the gate names the module" 1 "$(grep -c 'japan.rs' "$scratch/out")"

r="$scratch/decl"; mkrepo "$r"
echo 'pub mod set_discovery;' >>"$r/crates/dumpstar-ingest/src/lib.rs"
check "a declaration of a removed module fails the gate" 1 "$(run_gate "$r")"

r="$scratch/data"; mkrepo "$r"
echo '[]' >"$r/data/variants.json"
check "a removed Pokémon data file fails the gate" 1 "$(run_gate "$r")"

echo "==> §2 a clean tree passes"
r="$scratch/clean"; mkrepo "$r"
echo 'Star Wars: Unlimited' >"$r/README.md"
check "a tree without them passes" 0 "$(run_gate "$r")"

echo "==> §3 the real tree"
check "this repository passes" 0 "$(cd "$REPO_DIR" && bash "$GATE" >/dev/null 2>&1 && echo 0 || echo 1)"

[ "$fail" = 0 ] && echo "banned names: all assertions passed" || { echo "banned names: FAILED"; exit 1; }
