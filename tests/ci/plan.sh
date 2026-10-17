#!/usr/bin/env bash
# plan.sh — dumpstar's test plan: every suite declares its tier and what it covers, the use-case
# catalogue is the 228 UI intents in tests/ui/intents/, and the gate fails on a use case nothing
# covers or whose last cover a change deleted.
#
# This is Spira's mechanism (spira/plan-lint.sh, the `test-plan` binary, docs/test-plan/*.toml,
# `# covers: UC-<area>-NN` read by test-plan), ported — not a second one. The only dumpstar parts
# are where a use case is born and what a suite is:
#
#   use case   one tests/ui/intents/<name>.yaml, id on its second line: `# Use case: UC-<area>-NN`
#              (<area> = the file name up to its first `_`). docs/test-plan/<area>.toml is
#              GENERATED from them (`plan.sh generate`) and the gate fails when it has drifted.
#   suite      a shell gate under tests/ (header `# tier: T0..T4`, `# covers: <paths> [UC-…]`),
#              or a replay implementation tests/ui/implementations/<intent>.ts, whose line
#              `// covers: UC-…` names the intent it replays. An intent with an implementation is
#              tier T3; one without is replayed only by the Claude Vision harness, outside CI: T4.
#
#   plan.sh lint                 headers, ids, catalogue drift, gaps; exit 1 naming each. A MISSING header
#                                (intent id, suite tier/covers, implementation covers) FAILS, naming the
#                                fix command; DUMPSTAR_PLAN_REPORT_ONLY=1 prints it as "report-only:" instead.
#                                Authoring a test or intent: add its headers, then run
#                                `plan.sh assign && plan.sh generate && plan.sh report`.
#   plan.sh orphans [<ref>]      fail on a use case whose last cover was deleted since <ref>
#                                (default $DUMPSTAR_PLAN_BASE, origin/main, then main)
#   plan.sh generate             rewrite docs/test-plan/*.toml from the intents
#   plan.sh assign               give a new intent its next id
#   plan.sh report               write docs/test-plan/coverage.{json,md}: every intent, its covering suites
#   plan.sh suites [--ref <ref>] the suites as JSON, for test-plan
#
# The binary is `$SPIRA_TEST_PLAN_BIN`, else `test-plan` on PATH, else the installed Spira release, else
# the pinned copy in tools/test-plan, built on first use (`plan.sh bin` prints the path).
# It is never skipped: a missing binary is a failure, not a clean pass.
#
# tier: T1
# covers: tests/ci/plan.sh docs/test-plan/* tests/ui/intents/* tests/ui/implementations/*
set -uo pipefail
# Glob and sort order is the catalogue's row order: pin it, or the committed toml drifts per locale.
export LC_ALL=C
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
CAT="$ROOT/docs/test-plan"
INTENTS="tests/ui/intents"
IMPLS="tests/ui/implementations"
UC_RE='UC-[a-z0-9]+-[0-9]+'
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

test_plan_bin() {
    if [ -n "${SPIRA_TEST_PLAN_BIN:-}" ] && [ -x "$SPIRA_TEST_PLAN_BIN" ]; then printf '%s' "$SPIRA_TEST_PLAN_BIN"; return 0; fi
    command -v test-plan 2>/dev/null && return 0
    [ -x /home/ryan/spira/spira-releases/current/bin/test-plan ] && { printf '%s' /home/ryan/spira/spira-releases/current/bin/test-plan; return 0; }
    # None installed: build the pinned copy (tools/test-plan, UPSTREAM names the Spira commit).
    local out="${DUMPSTAR_TOOLS_TARGET:-$ROOT/target/test-plan}"
    if [ ! -x "$out/debug/test-plan" ]; then
        cargo build --locked -q --manifest-path "$ROOT/tools/test-plan/Cargo.toml" --target-dir "$out" >&2 || {
            echo "plan.sh: cannot build tools/test-plan — refusing to report clean" >&2; return 1; }
    fi
    printf '%s' "$out/debug/test-plan"
}

# Shell gates that are suites. A library sourced by gates is not one.
is_suite_sh() {
    case "$1" in
        tests/lib/*_test.sh) return 0 ;;
        tests/lib/test-jwks.sh) return 0 ;;
        tests/lib/*) return 1 ;;
        tests/visual/approval-guard.sh | tests/visual/failure-summary.sh | tests/visual/playwright.sh) return 1 ;;
        tests/ui/fixtures/build.sh) return 1 ;;
        tests/*.sh) return 0 ;;
    esac
    return 1
}

# header <file> tier|covers — the first `# tier:` / `# covers:` line in the leading comment block
# (a covers line folds its `#   `-indented continuations), as Spira's suite-select reads it.
header() {
    awk -v key="$2" '
        NR > 1 && !/^#/ && !/^[[:space:]]*$/ { exit }
        done == 0 && $0 ~ "^# *" key ":" { sub("^# *" key ": *", ""); out = $0; on = 1; next }
        on && /^#   +[^ ]/ { sub("^# +", ""); out = out " " $0; next }
        on { done = 1; on = 0 }
        END { print out }' "$1"
}

json_str() { local s="$1"; s="${s//\\/\\\\}"; s="${s//\"/\\\"}"; printf '"%s"' "$s"; }

# suites_json [<ref>] — every suite as {"path","tier","covers":[…]}.
suites_json() {
    local ref="${1:-}" first=1 f tier cov tok tmp files t2 id c
    tmp="$(mktemp -d -p "$WORK")"
    if [ -n "$ref" ]; then
        files="$(git -C "$ROOT" ls-tree -r --name-only "$ref" -- tests)"
    else
        files="$(cd "$ROOT" && find tests -type f -not -path "*/node_modules/*" | sort)"
    fi
    printf '['
    while IFS= read -r f; do
        case "$f" in
            *.sh) is_suite_sh "$f" || continue ;;
            "$IMPLS"/*.ts) ;;
            *) continue ;;
        esac
        if [ -n "$ref" ]; then git -C "$ROOT" show "$ref:$f" > "$tmp/x" 2>/dev/null || continue; else cp "$ROOT/$f" "$tmp/x"; fi
        if [ "${f%.ts}" != "$f" ]; then
            tier="T3"; cov="$(grep -E '^[[:space:]]*// covers:' "$tmp/x" | grep -oE "$UC_RE" | sort -u | tr '\n' ' ')"
        else
            tier="$(header "$tmp/x" tier)"; cov="$(header "$tmp/x" covers)"
        fi
        [ "$first" = 1 ] && first=0 || printf ','
        printf '{"path":%s,"tier":' "$(json_str "$f")"
        if [ -n "$tier" ]; then json_str "$tier"; else printf null; fi
        printf ',"covers":['
        c=1
        for tok in $cov; do [ "$c" = 1 ] && c=0 || printf ','; json_str "$tok"; done
        printf ']}'
    done <<< "$files"
    printf ']\n'
}

# intent_rows -> "<name>\t<uc-id or ->\t<statement>" for every intent, from the working tree.
# Bytes, not characters (LC_ALL=C, mawk and gawk alike); iconv -c drops a character the cut split.
# Dropping it is the point, so its status 1 for "skipped invalid input" is success: glibc 2.41+
# reports it (2.36 does not), and under pipefail it made lint fail with nothing printed.
intent_rows() {
    (cd "$ROOT/$INTENTS" && awk -v re="^# Use case: ($UC_RE) *\$" '
        function flush() { if (name != "") { gsub(/[\\"]/, "", s); if (length(s) > 220) s = substr(s, 1, 217) "..."; printf "%s\t%s\t%s\n", name, (id == "" ? "-" : id), s } }
        FNR == 1 { flush(); name = FILENAME; sub(/\.yaml$/, "", name); id = ""; s = ""; on = 0 }
        FNR == 2 && match($0, re) { id = $0; sub(/^# Use case: /, "", id); sub(/ +$/, "", id) }
        /^description:/ { on = 1; next }
        on && /^[^ ]/ { on = 0 }
        on { sub(/^ +/, ""); s = s (s ? " " : "") $0 }
        END { flush() }' *.yaml) | { iconv -c -f UTF-8 -t UTF-8; [ $? -le 1 ]; }
}

# missing <msg> <fix> — a missing header fails, naming the fix command. DUMPSTAR_PLAN_REPORT_ONLY=1
# downgrades it to a printed "report-only:" line (an escape hatch, not the default).
missing() {
    if [ "${DUMPSTAR_PLAN_REPORT_ONLY:-}" = 1 ]; then echo "report-only: $1"; return 0; fi
    echo "$1 — fix: $2"; return 1
}

area_of() { local n="${1%%_*}"; printf '%s' "$n"; }

cmd_assign() {
    local rows n id st area next f
    rows="$(intent_rows)"
    while IFS=$'\t' read -r n id st; do
        [ "$id" = "-" ] || continue
        area="$(area_of "$n")"
        next="$(printf '%s\n' "$rows" | awk -F'\t' -v a="UC-$area-" 'index($2, a) == 1 { sub(a, "", $2); if ($2 + 0 > m) m = $2 + 0 } END { printf "%02d", m + 1 }')"
        f="$ROOT/$INTENTS/$n.yaml"
        sed -i "1a # Use case: UC-$area-$next" "$f"
        rows="$(printf '%s\n' "$rows" | awk -F'\t' -v OFS='\t' -v n="$n" -v i="UC-$area-$next" '$1 == n { $2 = i } 1')"
        echo "assigned UC-$area-$next to $n"
    done <<< "$rows"
}

# The catalogue: T3 where a replay implementation exists, T4 where only the vision harness runs it.
cmd_generate() {
    local n id st area tier outdir="${1:-$CAT}" a
    mkdir -p "$outdir"; rm -f "$outdir"/*.toml
    while IFS=$'\t' read -r n id st; do
        [ "$id" != "-" ] || { [ "${DUMPSTAR_PLAN_REPORT_ONLY:-}" != 1 ] || continue
            echo "plan.sh: intent $n has no '# Use case:' line — run plan.sh assign" >&2; return 1; }
        a="$(area_of "$n")"
        tier=T4; [ -f "$ROOT/$IMPLS/$n.ts" ] && tier=T3
        [ -f "$outdir/$a.toml" ] || printf '# GENERATED by tests/ci/plan.sh from tests/ui/intents/*.yaml — do not edit.\napi_version = "test-plan/v1"\narea = "%s"\n' "$a" > "$outdir/$a.toml"
        printf '\n[[use_case]]\nid = "%s"\ntier = "%s"\nstatement = "%s (%s)"\n' "$id" "$tier" "$st" "$n" >> "$outdir/$a.toml"
    done < <(intent_rows)
}

cmd_report() {
    local tp suites; tp="$(test_plan_bin)" || return 1
    suites="$(mktemp -p "$WORK")"
    suites_json > "$suites"
    "$tp" write-matrix --catalogue-dir "$CAT" --suites "$suites" --out-json "$CAT/coverage.json" --out-md "$CAT/coverage.md"
}

cmd_lint() {
    local tp bad=0 suites f tier cov tok n id want tmp rows; tp="$(test_plan_bin)" || return 1
    [ -d "$ROOT/$INTENTS" ] && [ -n "$(ls "$ROOT/$INTENTS")" ] || { echo "plan.sh: no intents — refusing to report clean" >&2; return 3; }
    tmp="$(mktemp -d -p "$WORK")"

    # 1. every intent has a unique id, and the committed catalogue is what the intents generate.
    intent_rows | awk -F'\t' '$2 == "-" { print "intent " $1 " has no # Use case: line"; next }
        seen[$2]++ { print "intent " $1 ": duplicate id " $2; dup = 1 } END { exit dup }' > "$tmp/ids" && :
    # A failure here must say so: bad=1 with nothing printed is a red gate nobody can read.
    [ $? = 0 ] || { grep -q 'duplicate id' "$tmp/ids" || echo "plan.sh: reading $INTENTS failed"; bad=1; }
    while IFS= read -r f; do
        case "$f" in *"has no # Use case:"*) missing "$f" "bash tests/ci/plan.sh assign && bash tests/ci/plan.sh generate && bash tests/ci/plan.sh report" || bad=1 ;; *) echo "$f" ;; esac
    done < "$tmp/ids"
    if cmd_generate "$tmp/cat" 2> "$tmp/err"; then
        drift="$(diff -rq "$tmp/cat" "$CAT" 2>/dev/null | grep -v 'coverage\.' || true)"
        [ -n "$drift" ] && {
            echo "docs/test-plan/*.toml has drifted from tests/ui/intents/ — run: bash tests/ci/plan.sh generate"; bad=1; }
    else cat "$tmp/err"; bad=1; fi

    # 2. every shell suite declares tier and covers.
    while IFS= read -r f; do
        is_suite_sh "$f" || continue
        tier="$(header "$ROOT/$f" tier)"; cov="$(header "$ROOT/$f" covers)"
        [ -n "$tier" ] || missing "$f: missing # tier:" "add '# tier: T0..T4' to its leading comment block" || bad=1
        [ -n "$cov" ] || missing "$f: missing # covers:" "add '# covers: <paths or UC-…>' to its leading comment block" || bad=1
        for tok in $(printf '%s\n' $cov | grep -E "^$UC_RE\$"); do
            grep -qF "id = \"$tok\"" "$CAT"/*.toml || { echo "$f: unknown UC id on # covers: $tok"; bad=1; }
        done
    done < <(cd "$ROOT" && find tests -name '*.sh' -not -path '*/node_modules/*' | sort)

    # 3. every replay implementation names exactly its own intent's use case.
    rows="$(intent_rows)"
    for f in "$ROOT/$IMPLS"/*.ts; do
        n="$(basename "$f" .ts)"
        id="$(printf '%s\n' "$rows" | awk -F'\t' -v n="$n" '$1 == n { print $2 }')"
        [ -n "$id" ] || { echo "$IMPLS/$n.ts: no intent $INTENTS/$n.yaml"; bad=1; continue; }
        grep -qE "^[[:space:]]*// covers:.*\\b$id\\b" "$f" || missing "$IMPLS/$n.ts: missing '// covers: $id'" "add the line '// covers: $id' at the top of the file" || bad=1
    done

    # 4. test-plan: the catalogue validates, and no T0-T3 use case is without a cover.
    suites_json > "$tmp/suites.json"
    "$tp" validate --catalogue-dir "$CAT" --suites "$tmp/suites.json" || bad=1
    "$tp" gaps --catalogue-dir "$CAT" --suites "$tmp/suites.json" || bad=1

    # 5. the committed coverage report is current.
    if "$tp" write-matrix --catalogue-dir "$CAT" --suites "$tmp/suites.json" --out-json "$tmp/c.json" --out-md "$tmp/c.md" >/dev/null 2>&1; then
        { cmp -s "$tmp/c.md" "$CAT/coverage.md" && cmp -s "$tmp/c.json" "$CAT/coverage.json"; } || {
            echo "docs/test-plan/coverage.* is stale — run: bash tests/ci/plan.sh report"; bad=1; }
    else echo "test-plan write-matrix failed"; bad=1; fi

    [ "$bad" = 0 ] && echo "plan: clean — every suite declares tier and covers, every intent has an id and no gaps"
    return "$bad"
}

cmd_orphans() {
    local tp ref="${1:-}" tmp; tp="$(test_plan_bin)" || return 1
    if [ -z "$ref" ]; then
        for ref in "${DUMPSTAR_PLAN_BASE:-}" origin/main main; do
            [ -n "$ref" ] && git -C "$ROOT" rev-parse --verify -q "$ref^{commit}" >/dev/null && break
            ref=""
        done
    fi
    [ -n "$ref" ] || { echo "plan.sh orphans: no base ref (set DUMPSTAR_PLAN_BASE) — refusing to report clean" >&2; return 1; }
    tmp="$(mktemp -d -p "$WORK")"
    suites_json > "$tmp/cur.json"; suites_json "$ref" > "$tmp/prev.json"
    "$tp" orphans --catalogue-dir "$CAT" --suites "$tmp/cur.json" --prev-suites "$tmp/prev.json"
}

case "${1:-}" in
    lint) cmd_lint ;;
    orphans) shift; cmd_orphans "$@" ;;
    generate) cmd_generate ;;
    assign) cmd_assign ;;
    report) cmd_report ;;
    bin) test_plan_bin; echo ;;
    suites) if [ "${2:-}" = --ref ]; then suites_json "${3:?ref}"; else suites_json; fi ;;
    *) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac
