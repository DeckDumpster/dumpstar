#!/usr/bin/env bash
# tier: T1
# covers: deploy/render.sh deploy/render-lib.sh deploy/env/*.env deploy/dumpstar.container deploy/dumpstar-litestream.container
# tests/deploy/render_test.sh — an instance is the templates plus six declared parameters (sp-qgjpvj).
#
#   1. RENDERING PROD REPRODUCES TODAY'S FILES: byte-equal to tests/deploy/golden/prod/, the units
#      and env files as the install and scaffold paths wrote them before they went through the
#      templates. Secrets are paths: no env file carries a live value.
#   2. PROD VERSUS ANOTHER INSTANCE DIFFERS ONLY IN THE DECLARED PARAMETERS: the second render is
#      mapped back (each declared value to prod's) and must equal prod. A literal that should have
#      been a parameter survives the mapping and fails here. The OnFailure= alerting gate is the
#      one other difference, and it is asserted separately.
#   3. THE DIFF TEST CAN FAIL: a template with a hardcoded "prod" is rejected by the same check.
#
# Hermetic: sed and diff only.
#
#   bash tests/deploy/render_test.sh
set -euo pipefail
REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0; fail=0
ok() { echo "  PASS  $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL  $1"; fail=$((fail + 1)); }

R="$REPO_DIR/deploy/render.sh"
CHK=/home/ryangantt/dumpstar
bash "$R" "$WORK/prod" prod --checkout "$CHK"
if diff -r "$REPO_DIR/tests/deploy/golden/prod" "$WORK/prod" >"$WORK/d1"; then
	ok "prod renders byte-equal to the golden files"
else bad "prod differs from the golden files"; cat "$WORK/d1"; fi

if grep -E '^[^#]*(SECRET|COOKIE_KEY|PASSWORD|TOKEN)[A-Z_]*=[^ ]' "$WORK"/prod/*.env | grep -v CHANGE_ME >"$WORK/s"; then
	bad "an env file inlines a secret"; cat "$WORK/s"
else ok "no env file inlines a secret"; fi

# env-1, every declared parameter different, alerting set as prod's so the gate is not in play.
export DUMPSTAR_ALERT_INSTANCES="prod env-1"
bash "$R" "$WORK/env1" env-1 --hostname env-1.example.test --port 9001 --volume vol-env-1 \
	--secrets /srv/env-1/secrets --s3-prefix pre/env-1 --checkout "$CHK"
# Map prod's render forward to env-1's values; a literal the template should have parameterised
# stays "prod" in the env-1 render and is then the difference.
forward() { # forward <dir>
	local f
	for f in "$1"/*; do
		sed -e 's|%h/\.config/dumpstar/prod|/srv/env-1/secrets|g' -e 's|\bprod/|pre/env-1/|g' \
			-e 's|star\.dumpster\.cards|env-1.example.test|g' -e 's|:8091:|:9001:|g' \
			-e 's|dumpstar-prod-data|vol-env-1|g' -e 's|\bprod\b|env-1|g' "$f" >"$WORK/m.$$" &&
			mv "$WORK/m.$$" "$f"
	done
	for f in "$1"/*prod.container; do [ -e "$f" ] && mv "$f" "$(dirname "$f")/$(basename "$f" | sed "s/prod/env-1/")"; done
	return 0
}
cp -r "$WORK/prod" "$WORK/prodf"; forward "$WORK/prodf"
if diff -r "$WORK/prodf" "$WORK/env1" >"$WORK/d2"; then
	ok "prod and env-1 differ only in the declared parameters"
else bad "undeclared difference between prod and env-1"; cat "$WORK/d2"; fi

# The alerting gate: by default env-1 is silent (no OnFailure=), and nothing else changes.
unset DUMPSTAR_ALERT_INSTANCES
bash "$R" "$WORK/env1g" env-1 --hostname env-1.example.test --port 9001 --volume vol-env-1 \
	--secrets /srv/env-1/secrets --s3-prefix pre/env-1 --checkout "$CHK"
diff -r "$WORK/prodf" "$WORK/env1g" | grep '^[<>]' | grep -v '^< OnFailure=' >"$WORK/d3" || true
if [ ! -s "$WORK/d3" ]; then ok "the only other difference is the OnFailure= alerting gate"
else bad "env-1 differs beyond the alerting gate"; cat "$WORK/d3"; fi

# The check must be seen to fail: plant a hardcoded literal in a copy of the templates.
cp -r "$REPO_DIR" "$WORK/planted" 2>/dev/null || true
sed -i 's|^Environment=DUMPSTAR_HOME=/data|Environment=DUMPSTAR_HOME=/data\nEnvironment=DUMPSTAR_PEER=dumpstar-prod|' "$WORK/planted/deploy/dumpstar.container"
export DUMPSTAR_ALERT_INSTANCES="prod env-1"
bash "$WORK/planted/deploy/render.sh" "$WORK/pp" prod --checkout "$CHK"
bash "$WORK/planted/deploy/render.sh" "$WORK/pe" env-1 --hostname env-1.example.test --port 9001 \
	--volume vol-env-1 --secrets /srv/env-1/secrets --s3-prefix pre/env-1 --checkout "$CHK"
forward "$WORK/pp"
if diff -r "$WORK/pp" "$WORK/pe" >/dev/null; then bad "a planted hardcoded literal went unnoticed"
else ok "a hardcoded literal in a template is caught"; fi

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
