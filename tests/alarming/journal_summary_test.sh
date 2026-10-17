#!/usr/bin/env bash
# tier: T1
# covers: tests/alarming/journal_summary_test.sh deploy/ci.sh
# Unit test for deploy/journal-summary.sh and alert.sh's 900-byte budget (pd-pwk8).
#
# ── WHAT THIS EXISTS TO CATCH ───────────────────────────────────────────────
# A page that arrives and says nothing. Layer 2 fired correctly on 2026-08-12,
# reached the phone, and read:
#
#   ...org.opencontainers.image.source=https://github.com/benbjohnson/lite...
#   backup-check: STALE — the user registry: newest S3 replica write is 65h old
#   dumpstar-backup-check@prod.service: Main process exited, code=exited, status=1/FAILURE
#
# tests/alarming/run.sh proved the push HAPPENED, which is why the defect
# survived it: every layer fired and the message was useless. So this asserts
# on the CONTENT, against journal tails captured from the real units —
# fixtures/backup-check-podman.journal is the 2026-08-12 failure verbatim.
#
# Hermetic and sub-second: no podman, no systemd, no network beyond a local
# recorder (§5 puts a fake curl on PATH to see what alert.sh sent), so
# deploy/ci.sh runs it in the lint tier.
#
#   bash tests/alarming/journal_summary_test.sh
set -uo pipefail # NOT -e: a failed assertion must be reported, not fatal

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
FIX="${SCRIPT_DIR}/fixtures"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/dumpstar-jsum.XXXXXX")"
# §5 pushes through the real alert.sh, which now remembers what it sent
# (pd-hqdt). Keep that memory inside the temp dir: the real one under
# ~/.local/state belongs to the box's own alerting, and a test that wrote there
# could silence a genuine prod page for 24h.
export DUMPSTAR_ALERT_STATE_DIR="${WORK}/alert-state"
# And switch the window off, because §5 is arithmetic about the 900-byte budget
# and the suppression notice is paid for out of that same budget. With it armed,
# the 895-byte pad below no longer lands on the em-dash and the mid-character
# assertion would pass while testing nothing. The same cut WITH the notice is
# asserted in tests/alarming/alert_suppress_test.sh §7, which computes the
# budget rather than assuming it.
export DUMPSTAR_ALERT_SUPPRESS_SECONDS=0
# shellcheck disable=SC2329  # invoked via trap
cleanup() {
	local rc=$?
	rm -rf "$WORK"
	return $rc
}
trap cleanup EXIT

pass=0
fail=0
check() { # check <label> <expected> <actual>
	if [[ "$2" == "$3" ]]; then
		echo "  PASS  $1"
		pass=$((pass + 1))
	else
		echo "  FAIL  $1"
		echo "          expected: $2"
		echo "          actual:   $3"
		fail=$((fail + 1))
	fi
}
log() { printf '\n=== %s ===\n' "$*"; }

# The message Layer 2 would push for <unit>, given <fixture> as its journal tail.
summarise() { # summarise <fixture> <unit>
	bash "${REPO_DIR}/deploy/journal-summary.sh" "$2" <"${FIX}/$1"
}
# Substring assertions phrased so a failure prints a word, not a diff of a page.
has() { case "$1" in *"$2"*) echo yes ;; *) echo no ;; esac; }

# ── 1. the 2026-08-12 page, rewritten ───────────────────────────────────────
log "1. the real prod failure leads with the line that says what went wrong"
MSG="$(summarise backup-check-podman.journal dumpstar-backup-check@prod.service)"
printf '%s\n' "$MSG" | sed 's/^/    | /'

check "the first line IS the cause, with the unit and its exit status" \
	"dumpstar-backup-check@prod FAILED (exit 1) — backup-check: STALE — the user registry: newest S3 replica write is 66h old (> 36h threshold)" \
	"$(printf '%s\n' "$MSG" | head -n1)"
# The blob that used to be the page. Four separate assertions because each is a
# different source of noise and a filter can regress one at a time.
check "podman's OCI image metadata is gone" "no" "$(has "$MSG" 'org.opencontainers')"
check "the container id blob is gone" "no" "$(has "$MSG" 'container died')"
check "systemd's 'Main process exited' is gone" "no" "$(has "$MSG" 'Main process exited')"
check "systemd's 'Triggering OnFailure=' is gone" "no" "$(has "$MSG" 'Triggering OnFailure')"
check "the syslog prefixes are gone" "no" "$(has "$MSG" 'youthful_swartz[')"
# The service's own output still reaches the page — that is the half of the
# journal this whole exercise exists to keep.
check "the container's own output survives as context" "yes" "$(has "$MSG" 'min_txid')"
check "and the whole page fits the budget alert.sh enforces" "yes" \
	"$([ "$(printf '%s' "$MSG" | wc -c)" -le 900 ] && echo yes || echo no)"

# ── 2. a podman-backed unit whose failure is NOT the last line ──────────────
log "2. the sidecar: heartbeats collapse, the error is the headline"
MSG="$(summarise litestream-sidecar.journal dumpstar-litestream-prod.service)"
printf '%s\n' "$MSG" | cut -c1-140 | sed 's/^/    | /'

check "the headline is the error, not the newest heartbeat" "yes" \
	"$(has "$(printf '%s\n' "$MSG" | head -n1)" 'level=ERROR')"
check "and it still carries the unit and exit status" "yes" \
	"$(has "$(printf '%s\n' "$MSG" | head -n1)" 'dumpstar-litestream-prod FAILED (exit 1)')"
check "the sidecar's OCI metadata is gone too" "no" "$(has "$MSG" 'org.opencontainers')"
# 21 heartbeats in, at most a couple of collapsed representatives out. Without
# this the context block is a fifth of the page spent saying the same thing.
check "a second-by-second heartbeat does not fill the page" "yes" \
	"$([ "$(printf '%s\n' "$MSG" | grep -c 'replica sync' || true)" -le 3 ] && echo yes || echo no)"
check "the collapse says how many it stands for" "yes" "$(has "$MSG" '(x10)')"
check "the whole page fits the budget" "yes" \
	"$([ "$(printf '%s' "$MSG" | wc -c)" -le 900 ] && echo yes || echo no)"

# ── 3. a unit that failed without saying anything ───────────────────────────
log "3. no output of its own is still a usable page"
MSG="$(summarise silent-timeout.journal dumpstar-refresh@prod.service)"
printf '%s\n' "$MSG" | sed 's/^/    | /'
check "the body is not empty" "yes" "$([ -n "$MSG" ] && echo yes || echo no)"
check "it names the unit" "yes" "$(has "$MSG" 'dumpstar-refresh@prod FAILED')"
# No exit code exists for a timeout — systemd killed it. Say the word rather
# than inventing a number or leaving the reader with 'FAILED' alone.
check "and says how it failed, in systemd's own word" "yes" "$(has "$MSG" '(timeout)')"

# ── 4. a tail with no prefixes to read (journalctl -o cat) ──────────────────
log "4. the content filter stands alone when there is no prefix to parse"
MSG="$(summarise backup-check-cat.journal dumpstar-backup-check@prod.service)"
printf '%s\n' "$MSG" | sed 's/^/    | /'
check "the cause still leads" "yes" "$(has "$(printf '%s\n' "$MSG" | head -n1)" 'backup-check: STALE')"
check "the OCI metadata is still dropped" "no" "$(has "$MSG" 'org.opencontainers')"
check "the boilerplate is still dropped" "no" "$(has "$MSG" 'Failed with result')"

# ── 5. alert.sh spends the budget on the HEAD ───────────────────────────────
log "5. alert.sh keeps the first 900 bytes, not the last"
# A recorder stands in for Pushover: a `curl` first on PATH that appends every
# argument alert.sh passes it to a file. No server, no interpreter — the message
# is a --form-string argument, so the argument list IS what would be sent.
FAKEBIN="${WORK}/bin"
SENT="${WORK}/sent.log"
mkdir -p "$FAKEBIN"
cat >"${FAKEBIN}/curl" <<'CURL'
#!/usr/bin/env bash
printf 'REQUEST\n' >>"$SENT_LOG"
for a in "$@"; do printf '%s\n' "$a" >>"$SENT_LOG"; done
CURL
chmod +x "${FAKEBIN}/curl"
export SENT_LOG="$SENT"
push() { # push <message>
	: >"$SENT"
	PATH="${FAKEBIN}:${PATH}" PUSHOVER_TOKEN=t PUSHOVER_USER=u \
		PUSHOVER_API_URL="http://127.0.0.1:9/pushover" \
		bash "${REPO_DIR}/deploy/alert.sh" "gate" "$1" >/dev/null 2>&1
}

# 2 KB of message with a marker at each end: exactly the shape a journal tail
# had, and the reason the wrong end used to arrive.
LONG="CAUSE-IS-HERE $(head -c 2000 /dev/zero | tr '\0' 'x') BOILERPLATE-IS-HERE"
push "$LONG"
check "the push went out" "1" "$(grep -c '^REQUEST$' "$SENT" || true)"
check "the head of the message survived" "1" "$(grep -c 'CAUSE-IS-HERE' "$SENT" || true)"
check "the tail was what got trimmed" "0" "$(grep -c 'BOILERPLATE-IS-HERE' "$SENT" || true)"
check "and the reader is told it was trimmed" "1" "$(grep -c 'xxx\.\.\.' "$SENT" || true)"

# A message under the cap is passed through untouched — the trim must not cost
# a trailing character on every ordinary page. The em-dash is the point: every
# headline this repo writes carries one, the cut is by BYTES, and half a
# character is a request Pushover rejects outright.
push "short — and whole"
check "a short message arrives whole, em-dash included" "1" \
	"$(grep -cFx 'message=short — and whole' "$SENT" || true)"

# The same message cut exactly where the em-dash sits: three bytes, one of them
# the 900th. What must NOT arrive is the first byte of it on its own, so the
# recorded request has to be valid UTF-8 end to end.
PAD="$(head -c 895 /dev/zero | tr '\0' 'y')"
push "${PAD}—tail"
check "a cut that lands mid-character drops the character, not half of it" "yes" \
	"$(iconv -f UTF-8 -t UTF-8 "$SENT" >/dev/null 2>&1 && echo yes || echo no)"
check "and that message still arrived" "1" "$(grep -c 'yyy' "$SENT" || true)"

# ── 6. THE RATCHET: the shipped unit must go through the summariser ─────────
log "6. dumpstar-alert@.service pipes the tail through journal-summary.sh"
UNIT_FILE="${REPO_DIR}/deploy/dumpstar-alert@.service"
EXEC="$(grep '^ExecStart=' "$UNIT_FILE")"
check "the tail is summarised before it is pushed" "1" \
	"$(printf '%s\n' "$EXEC" | grep -c 'journal-summary.sh %i | bash' || true)"
check "and alert.sh is still what pushes it" "1" \
	"$(printf '%s\n' "$EXEC" | grep -c 'deploy/alert.sh' || true)"
# The defect verbatim: journalctl straight into alert.sh, no filter between.
check "journalctl never reaches alert.sh unfiltered again" "0" \
	"$(printf '%s\n' "$EXEC" | grep -c 'no-pager | bash [^|]*alert\.sh' || true)"

# ── RESULT ──────────────────────────────────────────────────────────────────
log "RESULT"
echo "  ${pass} passed, ${fail} failed"
[[ $fail -eq 0 ]] || exit 1
