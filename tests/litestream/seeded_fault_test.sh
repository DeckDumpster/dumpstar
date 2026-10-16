#!/usr/bin/env bash
# tier: T2
# covers: deploy/dumpstar-litestream.container
# Seeded-fault proof for the three S4 acceptance claims (sp-rhwo4u): every
# claim is run against the shipped tree (must hold) and then against a copy with
# one fault seeded in (must be caught), so each check is seen to fail first.
#
#   restore of a NON-FIRST tenant   tenant_replica_url must give every database
#                                   its own, full-stem prefix. Fault: the stem is
#                                   truncated to its ULID timestamp, so tenants
#                                   minted in the same window share a prefix.
#   a re-created handle             a new database_id never shares its
#                                   predecessor's replica; the CONTROL arm shows
#                                   handle-derived naming does (the old bug).
#                                   Fault: addressing that ignores the id.
#   a replica that stops advancing  backup-check must go STALE on it. Fault: the
#                                   staleness trip removed.
#   directory mode + watch + registry path entry in deploy/litestream.yml.
#                                   Fault: `watch: true` dropped / registry gone.
#
# Hermetic — no podman, no network. The container tier (drill.sh, recreate.sh,
# run.sh) proves the same claims end to end against a real sidecar and MinIO;
# this proves the CHECKS can fail, in seconds, in the lint tier.
#
#   bash tests/litestream/seeded_fault_test.sh
set -uo pipefail

REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
fails=0
say() { printf '%s\n' "$*"; }
verdict() { # verdict <ok|bad> <label>
	if [ "$1" = ok ]; then say "ok   $2"; else say "FAIL $2"; fails=$((fails + 1)); fi
}

# probe_addressing <root>: 0 when the addressing claims hold in <root>.
probe_addressing() (
	set -u
	export LITESTREAM_S3_BUCKET=b LITESTREAM_S3_PATH=pfx LITESTREAM_S3_REGION=r
	. "$1/deploy/litestream-lib.sh" 2>/dev/null || exit 2
	first=01J8ZZZZZZAAAAAAAAAAAAAAAA second=01J8ZZZZZZBBBBBBBBBBBBBBBB
	u1="$(tenant_replica_url "$first")" || exit 1
	u2="$(tenant_replica_url "$second")" || exit 1
	[ "$u1" != "$u2" ] || exit 1
	case "$u2" in */"$second".sqlite\?*) ;; *) exit 1 ;; esac
	exit 0
)

# probe_recreate <root>: a re-created handle gets a new id, hence a new prefix.
probe_recreate() (
	set -u
	export LITESTREAM_S3_BUCKET=b LITESTREAM_S3_PATH=pfx LITESTREAM_S3_REGION=r
	. "$1/deploy/litestream-lib.sh" 2>/dev/null || exit 2
	old=01J8ZZZZZZCCCCCCCCCCCCCCCC new=01J8ZZZZZZDDDDDDDDDDDDDDDD
	[ "$(tenant_replica_url "$old")" != "$(tenant_replica_url "$new")" ]
)

# The control arm: handle-derived naming, the old design. The same handle names
# the same prefix before and after re-creation, so the old account's replica is
# reachable by the new one. This must reproduce the leak.
control_handle_url() { printf 's3://b/pfx/%s.sqlite' "$1"; }
[ "$(control_handle_url alice)" = "$(control_handle_url alice)" ] \
	&& verdict ok "control arm: handle-derived naming reaches the predecessor's replica (old bug reproduced)" \
	|| verdict bad "control arm no longer reproduces the old bug"

probe_yml() { # 0 when dir mode + watch + registry path entry + snapshot + debug
	local f="$1/deploy/litestream.yml"
	grep -Eq '^[[:space:]]*-[[:space:]]+dir:' "$f" \
		&& grep -Eq '^[[:space:]]*watch:[[:space:]]*true' "$f" \
		&& grep -Eq '^[[:space:]]*-[[:space:]]+path:[[:space:]]*\$\{LITESTREAM_REGISTRY_DB\}' "$f" \
		&& grep -Eq '^snapshot:' "$f" \
		&& grep -Eq '^[[:space:]]*level:[[:space:]]*debug' "$f"
}

probe_stall() { # 0 when the correspondence matrix (incl. the stalled case) passes
	BACKUP_CHECK_SRC="$1/deploy/backup-check.sh" \
		bash "$REPO_DIR/tests/alarming/backup_correspondence_test.sh" >/dev/null 2>&1
}

mutant() { # mutant <name> <file> <perl-expr>: a copy of the tree with one fault
	local d="$TMP/$1"; mkdir -p "$d/deploy"
	cp "$REPO_DIR"/deploy/litestream-lib.sh "$REPO_DIR"/deploy/litestream.yml \
		"$REPO_DIR"/deploy/backup-check.sh "$d/deploy/"
	local before after
	before="$(cksum <"$d/deploy/$2")"
	perl -0pi -e "$3" "$d/deploy/$2"
	after="$(cksum <"$d/deploy/$2")"
	[ "$before" != "$after" ] || { say "FAIL mutant $1 changed nothing (stale pattern)"; fails=$((fails + 1)); }
	printf '%s' "$d"
}

# ── pass arm: the shipped tree holds every claim ────────────────────────────
for p in addressing recreate yml stall; do
	"probe_$p" "$REPO_DIR" && verdict ok "shipped tree: $p holds" || verdict bad "shipped tree: $p does not hold"
done

# ── fail arm: each seeded fault is caught ───────────────────────────────────
expect_caught() { # expect_caught <probe> <mutant-root> <label>
	if "probe_$1" "$2"; then verdict bad "NOT CAUGHT: $3"; else verdict ok "caught: $3"; fi
}
d="$(mutant trunc litestream-lib.sh 's|/\$\{1\}\.sqlite\?region|/\${1:0:10}.sqlite?region|')"
expect_caught addressing "$d" "non-first tenant: stem truncated to its timestamp, prefixes collide"
d="$(mutant fixedstem litestream-lib.sh 's|/\$\{1\}\.sqlite\?region|/tenant.sqlite?region|')"
expect_caught recreate "$d" "re-created handle: addressing ignores the database id"
d="$(mutant nowatch litestream.yml 's|^(\s*)watch:\s*true|$1watch: false|m')"
expect_caught yml "$d" "litestream.yml without watch: true"
d="$(mutant noreg litestream.yml 's|-\s+path:\s*\$\{LITESTREAM_REGISTRY_DB\}|- path: /tmp/x|')"
expect_caught yml "$d" "litestream.yml without the registry path entry"
d="$(mutant nostall backup-check.sh 's|-gt "\$SIDECAR_GRACE_SECONDS"|-gt 99999999999|')"
expect_caught stall "$d" "replica position stops advancing: staleness trip removed"

[ "$fails" -eq 0 ] && { say "=== seeded faults: all caught, shipped tree clean ==="; exit 0; }
say "=== ${fails} failed ==="; exit 1
