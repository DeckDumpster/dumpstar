#!/usr/bin/env bash
# tier: T1
# covers: tests/ci/plan.sh docs/test-plan/* tests/ui/implementations/*
# The use-case gate is seen to fail first (law-absence-needs-a-positive-control): in a scratch
# copy of the tree, deleting the only suite covering a fixture use case, or an intent's id,
# must fail tests/ci/plan.sh naming it; the unmodified copy is clean.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
ok()  { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
cd "$(dirname "$0")/../.."
SPIRA_TEST_PLAN_BIN="$(bash tests/ci/plan.sh bin)" || exit 1; export SPIRA_TEST_PLAN_BIN
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
tar -cf - --exclude=.git --exclude=node_modules --exclude=target . | tar -xf - -C "$T"
git init -q "$T" && git -C "$T" add -A && git -C "$T" -c user.name=t -c user.email=t@t commit -qm base
plan() { (cd "$T" && bash tests/ci/plan.sh "$@" 2>&1); }

out="$(plan lint)" && ok "unmodified tree is clean" || bad "unmodified tree is red: $out"

uc="$(sed -n '1s|^// covers: ||p' "$T/tests/ui/implementations/browse_grid_fits_viewport.ts")"
[ -n "$uc" ] || bad "fixture implementation names no use case"
rm "$T/tests/ui/implementations/browse_grid_fits_viewport.ts"
out="$(plan lint)" && bad "lint passed with $uc's only suite deleted" || \
    { case "$out" in *"$uc"*) ok "lint names $uc after its only suite is deleted" ;; *) bad "lint red but does not name $uc: $out" ;; esac; }
out="$(plan orphans HEAD)" && bad "orphans passed with $uc's only suite deleted" || \
    { case "$out" in *"$uc"*) ok "orphans names $uc" ;; *) bad "orphans red but does not name $uc: $out" ;; esac; }
git -C "$T" checkout -q -- .

sed -i '1d;2d' "$T/tests/ci/select_test.sh"; sed -i '1i #!/usr/bin/env bash' "$T/tests/ci/select_test.sh"
# Enforcing by default: a suite without headers fails, naming the suite and the fix. The
# DUMPSTAR_PLAN_REPORT_ONLY=1 escape hatch prints the same gap and exits 0.
out="$(plan lint)" && bad "lint passed with a suite lacking its headers" || \
    { case "$out" in *select_test.sh*"missing # tier"*"fix: add '# tier:"*) ok "lint names a suite without headers and the fix" ;; *) bad "headerless suite not named with a fix: $out" ;; esac; }
out="$(DUMPSTAR_PLAN_REPORT_ONLY=1 plan lint)" || bad "report-only lint failed: $out"
case "$out" in *"report-only: tests/ci/select_test.sh: missing # tier"*) ok "report-only escape hatch exits 0 and names the suite" ;; *) bad "report-only did not name the suite: $out" ;; esac
git -C "$T" checkout -q -- .

# A new untagged intent fails and names the exact command.
printf 'name: x\ndescription: y\n' > "$T/tests/ui/intents/browse_new_untagged.yaml"
out="$(plan lint)" && bad "lint passed with an untagged new intent" || \
    { case "$out" in *browse_new_untagged*"plan.sh assign && bash tests/ci/plan.sh generate"*) ok "untagged intent fails with the fix command" ;; *) bad "untagged intent message lacks fix: $out" ;; esac; }
rm "$T/tests/ui/intents/browse_new_untagged.yaml"

# A new untagged test file fails and names the headers to add.
printf '#!/usr/bin/env bash\ntrue\n' > "$T/tests/new_untagged_test.sh"
out="$(plan lint)" && bad "lint passed with an untagged new test file" || \
    { case "$out" in *new_untagged_test.sh*"missing # covers"*"fix: add '# covers:"*) ok "untagged test file fails with the fix" ;; *) bad "untagged test file message lacks fix: $out" ;; esac; }
rm "$T/tests/new_untagged_test.sh"
git -C "$T" checkout -q -- .

[ "$(plan report >/dev/null; git -C "$T" status --short | wc -l)" = 0 ] && ok "coverage report is current" || bad "coverage report regenerates differently"
exit "$fail"
