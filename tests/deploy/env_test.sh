#!/usr/bin/env bash
# tier: T2
# covers: deploy/env.sh deploy/env-vm.sh
# tests/deploy/env_test.sh — the env tool (sp-8i63uo) against a stubbed AWS, curl and sleep.
#
#   1. UP, STATUS, UPDATE, DOWN WORK: up takes a free slot and prints the URL and how to sign in,
#      records the edge share; a second up takes the next slot; a taken --name is refused; update
#      swaps the build and refuses --ref/--tree (sp-b2pnhw); down frees the slot.
#   2. NO TERRAFORM: terraform on PATH is a tripwire that fails the run; none of the verbs call it.
#   3. THE TEARDOWN POSITIVE CONTROL: every resource class `up` creates is named in
#      DUMPSTAR_ENV_RESOURCES, and after `down` none of them is left (no VM, no volume, no
#      connector). The control can fail: a stub that leaks a volume is caught.
#   4. THE SHIPPED SCRIPT NEVER CARRIES A SECRET: the SSM command holds the secret's name, not its
#      value, and the origin port the tool uses is infra/edge's origin_service.
#   5. NO FALLBACK: a missing configuration value refuses the command.
#
# Hermetic: bash, jq, sed, tar.
#
#   bash tests/deploy/env_test.sh
set -euo pipefail
REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0; fail=0
ok() { echo "  PASS  $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL  $1"; fail=$((fail + 1)); }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (want '$2', got '$3')"; fi; }

STUB="$WORK/stub"; BIN="$WORK/bin"
mkdir -p "$STUB/instances" "$STUB/volumes" "$BIN"
: >"$STUB/ledger"; : >"$STUB/ssm.log"; echo 0 >"$STUB/seq"

cat >"$BIN/aws" <<'STUBEOF'
#!/usr/bin/env bash
# A tiny in-memory EC2/SSM. State lives in $STUB.
set -euo pipefail
shopt -s nullglob
[ "$1" = --profile ] && shift 2
[ "$1" = --region ] && shift 2
svc="$1" op="$2"; shift 2
args="$*"
next() { local n; n=$(($(cat "$STUB/seq") + 1)); echo "$n" >"$STUB/seq"; echo "$n"; }
case "$svc $op" in
"ec2 run-instances")
	n="$(next)"; id="i-$(printf '%08d' "$n")"
	tags="$(sed -E 's/.*ResourceType=instance,Tags=\[([^]]*)\].*/\1/' <<<"$args" | sed -E 's/\},\{/}\n{/g' |
		sed -E 's/\{Key=([^,]*),Value=(.*)\}/\1\t\2/' | jq -Rn '[inputs | split("\t") | {key: .[0], value: .[1]}] | from_entries')"
	grep -q 'DeleteOnTermination=true' <<<"$args" || { echo "root volume would outlive the VM" >&2; exit 1; }
	jq -n --arg id "$id" --argjson tags "$tags" --arg t "$(printf '2026-10-22T00:00:%02dZ' "$n")" \
		'{id: $id, state: "running", launch: $t, tags: $tags}' >"$STUB/instances/$id.json"
	: >"$STUB/volumes/vol-$id"
	jq -n --arg id "$id" '{} ' >/dev/null
	echo instance >>"$STUB/ledger"; echo volume >>"$STUB/ledger"
	[ -n "${STUB_LEAK_VOLUME:-}" ] && : >"$STUB/volumes/leak-$id" && echo volume >>"$STUB/ledger"
	echo "$id" ;;
"ec2 describe-instances")
	if grep -q -- '--filters' <<<"$args"; then
		slot="$(sed -E 's/.*Name=tag:dumpstar-env:slot,Values=([^ ]*).*/\1/' <<<"$args")"
		cat "$STUB"/instances/*.json /dev/null | jq -s --arg s "$slot" '{Reservations: [{Instances: [.[] | select(.tags["dumpstar-env:slot"] == $s and .state != "terminated") |
			{InstanceId: .id, LaunchTime: .launch, State: {Name: .state}, Tags: (.tags | to_entries | map({Key: .key, Value: .value}))}]}]}'
	else
		id="$(sed -E 's/.*--instance-ids ([^ ]*).*/\1/' <<<"$args")"
		case "$args" in
		*State.Name*) jq -r .state "$STUB/instances/$id.json" ;;
		*) jq -r '.tags["dumpstar-env:slot"] // "None"' "$STUB/instances/$id.json" ;;
		esac
	fi ;;
"ec2 create-tags")
	id="$(sed -E 's/.*--resources ([^ ]*).*/\1/' <<<"$args")"
	for kv in $(sed -E 's/.*--tags (.*)/\1/' <<<"$args"); do
		k="$(sed -E 's/Key=([^,]*),.*/\1/' <<<"$kv")"; v="$(sed -E 's/.*Value=(.*)/\1/' <<<"$kv")"
		jq --arg k "$k" --arg v "$v" '.tags[$k] = $v' "$STUB/instances/$id.json" >"$STUB/t" && mv "$STUB/t" "$STUB/instances/$id.json"
	done ;;
"ec2 terminate-instances")
	id="$(sed -E 's/.*--instance-ids ([^ ]*).*/\1/' <<<"$args")"
	jq '.state = "terminated"' "$STUB/instances/$id.json" >"$STUB/t" && mv "$STUB/t" "$STUB/instances/$id.json"
	rm -f "$STUB/volumes/vol-$id" ;;
"ec2 describe-volumes") ls "$STUB/volumes" | tr '\n' ' ' ;;
"ec2 delete-volume") rm -f "$STUB/volumes/$(sed -E 's/.*--volume-id ([^ ]*).*/\1/' <<<"$args")" ;;
"ssm describe-instance-information") echo Online ;;
"ssm send-command")
	cmd=""
	while [ $# -gt 0 ]; do [ "$1" = --cli-input-json ] && cmd="$2"; shift; done
	jq -c . <<<"$cmd" >>"$STUB/ssm.log"
	if grep -q "export DUMPSTAR_ENV_VERB=.up." <<<"$(jq -r ".Parameters.commands[0]" <<<"$cmd")"; then
		echo data >>"$STUB/ledger"; echo connector >>"$STUB/ledger"
	fi
	echo cmd-1 ;;
"ssm get-command-invocation") echo Success ;;
*) echo "stub aws: unhandled $svc $op" >&2; exit 1 ;;
esac
STUBEOF
printf '#!/usr/bin/env bash\necho 200\n' >"$BIN/curl"
printf '#!/usr/bin/env bash\nexit 0\n' >"$BIN/sleep"
printf '#!/usr/bin/env bash\necho "terraform was called: $*" >>"%s/terraform.called"; exit 1\n' "$WORK" >"$BIN/terraform"
chmod +x "$BIN"/*

export STUB PATH="$BIN:$PATH"
export DUMPSTAR_ENV_AWS_PROFILE=p DUMPSTAR_ENV_REGION=us-west-2 DUMPSTAR_ENV_AMI=ami-1 DUMPSTAR_ENV_SUBNET=subnet-1 \
	DUMPSTAR_ENV_SECURITY_GROUP=sg-1 DUMPSTAR_ENV_INSTANCE_TYPE=c7i.large DUMPSTAR_ENV_INSTANCE_PROFILE=envvm \
	DUMPSTAR_ENV_ACCESS_TEAM=team DUMPSTAR_ENV_CLOUDFLARED_IMAGE=docker.io/cloudflare/cloudflared:2026.1.0 \
	DUMPSTAR_ENV_OWNER=tester
ENV="$REPO_DIR/deploy/env.sh"
TREE=$(printf 'a%.0s' $(seq 40))
TREE2=$(printf 'b%.0s' $(seq 40))

out="$(bash "$ENV" up --image "tree:$TREE" 2>&1)" || { bad "up failed"; echo "$out"; }
grep -q 'url: https://env-1.test.dumpster.cards/' <<<"$out" && ok "up prints the slot URL" || { bad "up prints the slot URL"; echo "$out"; }
grep -q 'sign in: Pocket ID' <<<"$out" && ok "up says how to sign in" || bad "up says how to sign in"
grep -q 'edge share [0-9]*s' <<<"$out" && ok "up records the edge share" || bad "up records the edge share"
id1="$(basename "$(ls "$STUB"/instances/*.json | head -1)" .json)"
check "edge share is a tag on the VM" 1 "$(jq '.tags | has("dumpstar-env:edge-seconds")' "$STUB/instances/$id1.json" | grep -c true)"
check "the launch carries the pool tag the policy demands" round "$(jq -r '.tags["spill:owner"]' "$STUB/instances/$id1.json")"

out2="$(bash "$ENV" up --image "tree:$TREE" 2>&1)" && grep -q 'env-2.test' <<<"$out2" && ok "a second up takes the next slot" || bad "a second up takes the next slot"
bash "$ENV" up --image "tree:$TREE" --name env-1 >/dev/null 2>&1 && bad "a taken name is refused" || ok "a taken name is refused"
bash "$ENV" up --image "tree:$TREE" --name env-9 >/dev/null 2>&1 && bad "an unknown slot is refused" || ok "an unknown slot is refused"
bash "$ENV" up --image "tree:abc" >/dev/null 2>&1 && bad "a short tree is refused" || ok "a short tree is refused"
st="$(bash "$ENV" status)"; grep -qE "^env-1 +tester.*${TREE}" <<<"$st" && ok "status lists owner, build and URL" || bad "status lists owner, build and URL"
grep -q '^env-3 *free' <<<"$(bash "$ENV" status)" && ok "status shows the free slot" || bad "status shows the free slot"

bash "$ENV" update env-1 --image "tree:$TREE2" >/dev/null && ok "update succeeds" || bad "update succeeds"
check "update records the new build" "tree:$TREE2" "$(jq -r '.tags["dumpstar-env:build"]' "$STUB/instances/$id1.json")"
bash "$ENV" update env-1 --ref main >/dev/null 2>&1 && bad "update --ref is refused until sp-b2pnhw" || ok "update --ref is refused until sp-b2pnhw"
check "update neither launches nor terminates" 2 "$(grep -c instance "$STUB/ledger")"

# 3. The teardown positive control.
created="$(sort -u "$STUB/ledger" | tr '\n' ' ')"
declared="$(bash -c ". '$REPO_DIR/deploy/env.sh' >/dev/null 2>&1 || true" ; sed -n 's/^DUMPSTAR_ENV_RESOURCES="\(.*\)"/\1/p' "$ENV")"
check "every resource class up creates is declared" "$(tr ' ' '\n' <<<"$declared" | sort | tr '\n' ' ')" "$created"
bash "$ENV" down env-1 >/dev/null && bash "$ENV" down env-2 >/dev/null && ok "down succeeds" || bad "down succeeds"
check "down leaves no live VM" 0 "$(jq -s '[.[] | select(.state != "terminated")] | length' "$STUB"/instances/*.json)"
check "down leaves no volume" 0 "$(ls "$STUB/volumes" | wc -l)"
grep -q '^env-1 *free' <<<"$(bash "$ENV" status)" && ok "down frees the slot" || bad "down frees the slot"
[ "$(grep -c "DUMPSTAR_ENV_VERB='stop'" "$STUB/ssm.log")" = 2 ] && ok "down stops the connector first" || bad "down stops the connector first"
bash "$ENV" down env-1 >/dev/null 2>&1 && bad "down on a free slot is refused" || ok "down on a free slot is refused"

# The control can fail: a leaked volume is removed by down only because down looks for it.
STUB_LEAK_VOLUME=1 bash "$ENV" up --image "tree:$TREE" >/dev/null
bash "$ENV" down env-1 >/dev/null
check "a volume that outlived its VM is deleted by down" 0 "$(ls "$STUB/volumes" | wc -l)"

# 2. No Terraform.
[ ! -e "$WORK/terraform.called" ] && ok "no verb ran Terraform" || { bad "a verb ran Terraform"; cat "$WORK/terraform.called"; }

# 4. No secret in the shipped script; port is the edge's origin_service.
grep -q 'dumpstar/env-1/edge' "$STUB/ssm.log" && ok "the command names the secret" || bad "the command names the secret"
grep -qE "(TUNNEL_TOKEN|CLIENT_SECRET)=[A-Za-z0-9]{8,}" "$STUB/ssm.log" && bad "a secret value is in the SSM command" || ok "no secret value is in the SSM command"
want="$(sed -n 's|.*default *= *"http://127.0.0.1:\([0-9]*\)".*|\1|p' "$REPO_DIR/infra/edge/variables.tf")"
grep -q "export DUMPSTAR_ENV_PORT='${want}'" "$STUB/ssm.log" && ok "the origin port is infra/edge's origin_service" || bad "the origin port drifted from infra/edge"

# 5. No fallback.
(unset DUMPSTAR_ENV_AMI; bash "$ENV" status >/dev/null 2>&1) && bad "a missing value is refused" || ok "a missing value is refused"

echo "env_test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
