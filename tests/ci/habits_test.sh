#!/usr/bin/env bash
# tier: T0
# covers: .spira/habits.toml
# Guard (sp-s5im2k): .spira/habits.toml parses, declares the four habits with
# name/glob/run/budget_ms, and the rustfmt habit invokes the tool and edition CI
# runs (`cargo fmt --check`, deploy/ci.sh; edition from Cargo.toml).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
f=.spira/habits.toml
fail=0
bad() { echo "FAIL: $*" >&2; fail=1; }

names="$(awk '/^\[\[habit\]\]/{n++} END{print n+0}' "$f")"
[[ "$names" -eq 4 ]] || bad "expected 4 [[habit]] entries, found $names"

# Split into one block per habit and check each field.
block=""
check() {
  [[ "$started" -eq 1 ]] || return 0
  local n
  n="$(sed -n 's/^name = "\(.*\)"/\1/p' <<<"$block")"
  [[ -n "$n" ]] || { bad "a habit has no name"; return; }
  for k in glob run budget_ms; do
    grep -q "^$k = " <<<"$block" || bad "habit $n lacks $k"
  done
  grep -q '^run = .*{file}' <<<"$block" || bad "habit $n: run lacks {file}"
  grep -Eq '^budget_ms = [0-9]+$' <<<"$block" || bad "habit $n: budget_ms not an integer"
  seen+=" $n"
}
seen=""; started=0
while IFS= read -r line; do
  if [[ "$line" == "[[habit]]" ]]; then check; started=1; block=""; else block+="$line"$'\n'; fi
done <"$f"
check
for want in rustfmt complexity shellcheck prettier; do
  [[ " $seen " == *" $want "* ]] || bad "missing habit $want"
done

# rustfmt habit == CI's formatter, same edition.
edition="$(sed -n 's/^edition = "\(.*\)"/\1/p' Cargo.toml | head -1)"
run="$(sed -n 's/^run = "\(rustfmt .*\)"/\1/p' "$f")"
[[ "$run" == "rustfmt --check --edition $edition {file}" ]] \
  || bad "rustfmt habit is '$run', want edition $edition"
grep -q 'cargo fmt --check' deploy/ci.sh || bad "deploy/ci.sh no longer runs cargo fmt --check"

((fail)) || echo "ok: habits.toml parses and matches CI"
exit "$fail"
