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

# The words are assembled so this file does not contain what it forbids.
ETB="E""TB"
TAG="Pok""émon TCG collection tracker"

r="$scratch/etb"; mkrepo "$r"
mkdir -p "$r/frontend/src/lib/components"
echo "<p>Sealed: ${ETB}</p>" >"$r/frontend/src/lib/components/Sealed.svelte"
check "a Pokémon sealed kind in the frontend fails the gate" 1 "$(run_gate "$r")"
check "…and the gate names the file" 1 "$(grep -c 'Sealed.svelte' "$scratch/out")"

r="$scratch/tagline"; mkrepo "$r"
mkdir -p "$r/frontend/src/routes"
echo "<p class=\"tagline\">A ${TAG}.</p>" >"$r/frontend/src/routes/+page.svelte"
check "the Pokémon tagline fails the gate" 1 "$(run_gate "$r")"

r="$scratch/example"; mkrepo "$r"
mkdir -p "$r/frontend/src/routes/search-help"
echo "{ q: 'pika""chu qty>=2' }" >"$r/frontend/src/routes/search-help/+page.svelte"
check "a Pokémon example in a page fails the gate" 1 "$(run_gate "$r")"

r="$scratch/crate-visible"; mkrepo "$r"
mkdir -p "$r/crates/dumpstar-server/src"
echo 'const ABOUT: &str = "A Pok''émon tracker";' >"$r/crates/dumpstar-server/src/lib.rs"
check "Pokémon wording in the server fails the gate" 1 "$(run_gate "$r")"

r="$scratch/crate-product"; mkrepo "$r"
echo "// 151 Elite"" Trainer Box" >"$r/crates/dumpstar-ingest/src/lib.rs"
check "a Pokémon sealed product name anywhere in crates fails the gate" 1 "$(run_gate "$r")"

# The removed card model's column names, assembled for the same reason.
SUPER="super""type"
r="$scratch/column"; mkrepo "$r"
mkdir -p "$r/crates/dumpstar-db/src"
echo "SELECT c.${SUPER} FROM cards c" >"$r/crates/dumpstar-db/src/q.rs"
check "a removed Pokémon column in crates/ fails the gate" 1 "$(run_gate "$r")"
check "…and the gate names the file and the column" 1 "$(grep -c "q.rs:1.*'${SUPER}'" "$scratch/out")"

r="$scratch/frontcolumn"; mkrepo "$r"
mkdir -p "$r/frontend/src/lib"
echo "export const t = row.ptcgo_code;" >"$r/frontend/src/lib/x.ts"
check "a removed Pokémon field in frontend/ fails the gate" 1 "$(run_gate "$r")"

r="$scratch/vendored"; mkrepo "$r"
mkdir -p "$r/frontend/src/lib/kit"
echo "/* ${SUPER} */" >"$r/frontend/src/lib/kit/tokens.css"
check "the vendored kit is not held to the card model" 0 "$(run_gate "$r")"

r="$scratch/detector"; mkrepo "$r"
mkdir -p "$r/crates/dumpstar-catalog/src"
echo "(\"cards\", \"${SUPER}\")," >"$r/crates/dumpstar-catalog/src/catalog.rs"
check "the catalog crate's legacy detector may name them" 0 "$(run_gate "$r")"

echo "==> §2 a clean tree passes"
r="$scratch/clean"; mkrepo "$r"
echo 'Star Wars: Unlimited' >"$r/README.md"
check "a tree without them passes" 0 "$(run_gate "$r")"

r="$scratch/card-model"; mkrepo "$r"
mkdir -p "$r/crates/dumpstar-db/src"
echo '// Pok''émon card model, owed to the catalog seam' >"$r/crates/dumpstar-db/src/search.rs"
check "the Pokémon card model outside the user-visible paths passes" 0 "$(run_gate "$r")"

echo "==> §3 the real tree"
# The gate's own output is shown when it fails, so CI names the offending file
# and the banned name (law-a-failure-carries-its-own-explanation).
gate_tree() { # gate_tree <dir> -> prints exit status, leaves output in $scratch/out
    local rc=0
    (cd "$1" && bash "$GATE" >"$scratch/out" 2>&1) || rc=$?
    echo "$rc"
}
rc="$(gate_tree "$REPO_DIR")"
check "this repository passes" 0 "$rc"
if [ "$rc" != 0 ]; then sed 's/^/        /' "$scratch/out"; fi

echo "==> §3b a failing tree's output names the file and the name"
r="$scratch/named"; mkrepo "$r"
echo "export ${OLD_ENV}_HOME=/tmp" >"$r/offender.sh"
git -C "$r" add -A
check "a tree with one banned name fails" 1 "$(gate_tree "$r")"
check "…the output names the file" 1 "$(grep -c 'offender.sh' "$scratch/out")"
check "…and the banned name" 1 "$(grep -c "'${OLD_ENV}'" "$scratch/out")"

[ "$fail" = 0 ] && echo "banned names: all assertions passed" || { echo "banned names: FAILED"; exit 1; }
