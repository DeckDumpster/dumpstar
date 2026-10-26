#!/usr/bin/env bash
# tier: T1
# covers: tests/alarming/backup_listing_test.sh deploy/backup-check.sh
# The freshness check lists every replica in a CONSTANT number of container
# starts, not one per tenant (sp-gpnbrp: 7 tenants x ~40s crossed the unit's 300s
# limit and paged DOWN with every backup fresh).
#
# Hermetic and sub-second: a stub podman on PATH counts `run` calls and answers
# in the shape the in-container listing loop prints.
#
#   bash tests/alarming/backup_listing_test.sh
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/../../deploy/backup-check.sh"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
fails=0
check() { if [ "$2" = "$3" ]; then printf '  ok    %s\n' "$1"; else printf '  FAIL  %s (got %s, want %s)\n' "$1" "$3" "$2"; fails=$((fails+1)); fi; }

mkdir -p "$TMP/bin"
cat >"$TMP/bin/podman" <<'STUB'
#!/usr/bin/env bash
echo run >>"$PODMAN_CALLS"
# args after the lone "sh" operand are the replica urls
while [ "$1" != sh ]; do shift; done; shift
for u in "$@"; do
    echo "@@@BEGIN $u"
    case "$u" in
        *bad*) echo "AccessDenied"; echo "@@@RC 1" ;;
        *) echo "0 2026-10-26T00:00:00Z $u"; echo "@@@RC 0" ;;
    esac
done
STUB
chmod +x "$TMP/bin/podman"
export PATH="$TMP/bin:$PATH" PODMAN_CALLS="$TMP/calls"

eval "$(awk '/^ltx_cache_key\(\)/' "$SRC")"
eval "$(awk '/^ltx_prefetch\(\) \{/,/^\}/' "$SRC")"
eval "$(awk '/^ltx_list\(\) \{/,/^\}/' "$SRC")"
CONF_DIR=/x INSTANCE=t LS_IMG=img

for n in 1 7 20; do
    : >"$PODMAN_CALLS"
    urls=(); for i in $(seq "$n"); do urls+=("s3://b/t$i"); done
    ltx_prefetch "${urls[@]}"
    ok=0; for u in "${urls[@]}"; do out="$(ltx_list "$u" w)"; grep -q "$u" <<<"$out" && ok=$((ok+1)); done
    check "$n tenants: one container start" 1 "$(wc -l <"$PODMAN_CALLS")"
    check "$n tenants: every listing served" "$n" "$ok"
done

ltx_prefetch s3://b/good s3://b/bad
check "a failing listing stays a failure" 1 "$(ltx_list s3://b/bad w >/dev/null 2>&1; echo $?)"
check "its neighbour still passes" 0 "$(ltx_list s3://b/good w >/dev/null 2>&1; echo $?)"

[ "$fails" -eq 0 ] || exit 1
