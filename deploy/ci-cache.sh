#!/usr/bin/env bash
# ci-cache.sh — skip a CI run when this EXACT tree already passed these EXACT tiers.
#
# The runner is serialised box-wide and the suite takes ~17 minutes, so re-testing
# a tree that has already been proven green is the most expensive no-op we have.
# A rebase that changes nothing, a docs commit on top of a tested branch, two PRs
# that converge on the same content — all of them pay full price today.
#
# WHAT THIS IS NOT: a way for an agent to declare its own work tested. Only CI
# writes to this cache (`dumpstar_cache_writable` refuses outside CI), because a
# commit status is just an HTTP POST and a cache entry is just a file — if either
# can be produced by whoever wants the green, the gate stops meaning "these tests
# passed on this tree" and starts meaning "someone said so".
#
# ── The five rules, each of which exists because its absence is a silent lie ──
#
# 1. THE TIER SET IS IN THE KEY. A docs-only PR runs the `lint` tier alone. If the
#    key were the tree alone, that pass would later satisfy a run that needed the
#    full suite — certifying eleven container gates that never executed. Keyed
#    together, a lint-only pass can only ever satisfy another lint-only run.
#
# 2. SUCCESSES ONLY. Never cache a failure. A flaky red cached is a tree that can
#    never go green again without a human clearing it, and this suite's flakiness
#    is exactly what motivated the cache.
#
# 3. A TTL, default 7 days. The tree is identical; the world is not. Toolchains,
#    base images, and the S3 the DR drills talk to all move underneath a green
#    result. The tree hash proves the inputs we version — not the ones we don't.
#
# 4. AN EPOCH. Bump DUMPSTAR_CACHE_EPOCH to invalidate every entry at once, for when
#    something outside the tree changes and you cannot enumerate what it touched.
#
# 5. A DIRTY TREE IS NOT CACHEABLE. `git status --porcelain` non-empty means the
#    hash does not describe what actually ran, so both lookup and store refuse.
set -euo pipefail

DUMPSTAR_CACHE_DIR="${DUMPSTAR_CACHE_DIR:-/workspaces/ci-tree-cache}"
DUMPSTAR_CACHE_TTL_DAYS="${DUMPSTAR_CACHE_TTL_DAYS:-7}"
DUMPSTAR_CACHE_EPOCH="${DUMPSTAR_CACHE_EPOCH:-1}"

dumpstar_cache_writable() { [ "${CI:-}" = "true" ] || [ "${DUMPSTAR_CACHE_FORCE_WRITABLE:-}" = "1" ]; }

# The tree of what is checked out. On a pull_request, actions/checkout leaves HEAD
# at the MERGE commit, so this is the tree that is actually tested — which is the
# thing worth keying on, not the branch head.
dumpstar_cache_tree() {
    [ -z "$(git status --porcelain 2>/dev/null)" ] || { echo "dirty" >&2; return 1; }
    git rev-parse 'HEAD^{tree}' 2>/dev/null
}

# key <tier...> — tiers are sorted so ordering cannot produce two keys for one plan.
dumpstar_cache_key() {
    local tree tiers
    tree="$(dumpstar_cache_tree)" || return 1
    tiers="$(printf '%s\n' "$@" | tr ' ' '\n' | grep -v '^$' | LC_ALL=C sort -u | tr '\n' ',')"
    [ -n "$tiers" ] || { echo "refusing to key an empty tier plan" >&2; return 1; }
    printf '%s\n%s\n%s\n' "$DUMPSTAR_CACHE_EPOCH" "$tree" "$tiers" | sha256sum | cut -d' ' -f1
}

# lookup <key> — exit 0 only on a hit that is present AND within the TTL.
dumpstar_cache_lookup() {
    local f="${DUMPSTAR_CACHE_DIR}/$1"
    [ -f "$f" ] || return 1
    if [ -n "$(find "$f" -mtime "+${DUMPSTAR_CACHE_TTL_DAYS}" 2>/dev/null)" ]; then
        echo "cache entry older than ${DUMPSTAR_CACHE_TTL_DAYS}d — ignoring" >&2
        return 1
    fi
    cat "$f"
}

# store <key> <description> — CI only, and only ever called after a PASS.
dumpstar_cache_store() {
    dumpstar_cache_writable || { echo "not CI — refusing to write the cache" >&2; return 1; }
    mkdir -p "$DUMPSTAR_CACHE_DIR"
    printf 'tree=%s\nkey=%s\nwhat=%s\n' "$(dumpstar_cache_tree)" "$1" "${2:-}" > "${DUMPSTAR_CACHE_DIR}/$1"
}

dumpstar_cache_prune() {
    [ -d "$DUMPSTAR_CACHE_DIR" ] || return 0
    find "$DUMPSTAR_CACHE_DIR" -type f -mtime "+${DUMPSTAR_CACHE_TTL_DAYS}" -delete 2>/dev/null || true
}

_cache_self_test() {
    local fails=0 tmp; tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' RETURN
    export DUMPSTAR_CACHE_DIR="${tmp}/cache" DUMPSTAR_CACHE_FORCE_WRITABLE=1
    ( cd "$tmp" && git init -q r && cd r && git config user.email t@t && git config user.name t \
        && echo one > a.txt && git add -A && git commit -qm one )
    cd "${tmp}/r"
    ok() { if [ "$2" = "$3" ]; then printf '  ok    %-52s %s\n' "$1" "$2"
           else printf '  FAIL  %-52s got=%s want=%s\n' "$1" "$2" "$3"; fails=$((fails+1)); fi }

    local kfull klint
    kfull="$(dumpstar_cache_key lint rust container)"; klint="$(dumpstar_cache_key lint)"

    ok "rule 1: a lint-only key differs from a full-suite key" \
       "$([ "$kfull" != "$klint" ] && echo differ || echo SAME)" "differ"
    ok "rule 1: tier ORDER does not change the key" \
       "$([ "$(dumpstar_cache_key container lint rust)" = "$kfull" ] && echo same || echo DIFFER)" "same"
    ok "a miss is a miss" "$(dumpstar_cache_lookup "$kfull" >/dev/null 2>&1 && echo hit || echo miss)" "miss"

    dumpstar_cache_store "$kfull" "self-test" >/dev/null
    ok "after a store, the same key hits" \
       "$(dumpstar_cache_lookup "$kfull" >/dev/null 2>&1 && echo hit || echo miss)" "hit"
    ok "rule 1: the lint-only key still MISSES (full pass cannot certify it)" \
       "$(dumpstar_cache_lookup "$klint" >/dev/null 2>&1 && echo hit || echo miss)" "miss"

    echo two > b.txt && git add -A && git commit -qm two
    ok "a changed tree misses" \
       "$(dumpstar_cache_lookup "$(dumpstar_cache_key lint rust container)" >/dev/null 2>&1 && echo hit || echo miss)" "miss"
    git revert -q --no-edit HEAD 2>/dev/null || { git reset -q --hard HEAD~1; }
    ok "reverting back to the proven tree HITS again" \
       "$(dumpstar_cache_lookup "$(dumpstar_cache_key lint rust container)" >/dev/null 2>&1 && echo hit || echo miss)" "hit"

    ok "rule 3: an entry past its TTL is ignored" \
       "$(touch -d '30 days ago' "${DUMPSTAR_CACHE_DIR}/${kfull}"; \
          dumpstar_cache_lookup "$kfull" >/dev/null 2>&1 && echo hit || echo miss)" "miss"
    touch "${DUMPSTAR_CACHE_DIR}/${kfull}"
    ok "rule 4: bumping the epoch invalidates everything" \
       "$(DUMPSTAR_CACHE_EPOCH=99 dumpstar_cache_lookup "$(DUMPSTAR_CACHE_EPOCH=99 dumpstar_cache_key lint rust container)" \
          >/dev/null 2>&1 && echo hit || echo miss)" "miss"

    echo dirty > c.txt
    ok "rule 5: a dirty tree cannot be keyed at all" \
       "$(dumpstar_cache_key lint >/dev/null 2>&1 && echo keyed || echo refused)" "refused"
    rm -f c.txt

    ok "rule 2/CI-only: a store outside CI is refused" \
       "$(env -u DUMPSTAR_CACHE_FORCE_WRITABLE CI=false bash -c '. '"$OLDPWD"'/deploy/ci-cache.sh 2>/dev/null; \
          DUMPSTAR_CACHE_DIR='"$DUMPSTAR_CACHE_DIR"' dumpstar_cache_store zzz x >/dev/null 2>&1 && echo wrote || echo refused')" "refused"
    ok "an empty tier plan is refused" \
       "$(dumpstar_cache_key "" >/dev/null 2>&1 && echo keyed || echo refused)" "refused"

    [ "$fails" -eq 0 ] && { echo "ALL PASS"; return 0; }
    echo "$fails FAILED"; return 1
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    case "${1:-}" in
        --self-test) OLDPWD="$(cd "$(dirname "$0")/.." && pwd)"; export OLDPWD; _cache_self_test; exit $? ;;
        key)    shift; dumpstar_cache_key "$@" ;;
        lookup) shift; dumpstar_cache_lookup "$@" ;;
        store)  shift; dumpstar_cache_store "$@" ;;
        prune)  dumpstar_cache_prune ;;
        *) echo "usage: ci-cache.sh {key <tier...>|lookup <key>|store <key> <what>|prune|--self-test}" >&2; exit 2 ;;
    esac
fi
