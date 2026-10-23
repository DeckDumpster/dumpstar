#!/usr/bin/env bash
# tier: T2
# covers: deploy/env.sh deploy/env-vm.sh deploy/realenv-reap.service deploy/realenv-reap.timer
# tests/deploy/env_test.sh — the env tool (sp-8i63uo) against a stubbed AWS, curl and sleep.
#
#   1. UP, STATUS, UPDATE, DOWN WORK: up takes a free slot and prints the URL and how to sign in,
#      records the edge share; a second up takes the next slot; a taken --name is refused; update
#      swaps the build; down frees the slot.
#   2. NO TERRAFORM: terraform on PATH is a tripwire that fails the run; none of the verbs call it.
#   3. THE TEARDOWN POSITIVE CONTROL: every resource class `up` creates is named in
#      DUMPSTAR_ENV_RESOURCES, and after `down` none of them is left (no VM, no volume, no
#      connector). The control can fail: a stub that leaks a volume is caught.
#   4. THE SHIPPED SCRIPT NEVER CARRIES A SECRET: the SSM command holds the secret's name, not its
#      value, and the origin port the tool uses is infra/edge's origin_service.
#   6. THE REAPER (sp-4wy00r): past 2 h an environment is gone however busy; idle 30 min with a
#      readable counter it is gone; active it survives; with an unreadable counter only the TTL
#      applies and status says "activity unknown". up and update count as activity.
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
"ec2 describe-images") echo "${STUB_AMI_SNAPSHOT_GB:-8}" ;;
"ec2 run-instances")
	sed -E 's/.*VolumeSize=([0-9]+).*/\1/' <<<"$args" >"$STUB/last-root-gb"
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
	if [ -n "${STUB_SSM_DENY:-}" ]; then
		echo "An error occurred (AccessDeniedException) when calling the SendCommand operation: not authorized to perform: ssm:SendCommand" >&2; exit 254
	fi
	if grep -q "export DUMPSTAR_ENV_VERB=.up." <<<"$(jq -r ".Parameters.commands[0]" <<<"$cmd")"; then
		echo data >>"$STUB/ledger"; echo connector >>"$STUB/ledger"
	fi
	echo cmd-1 ;;
"ssm get-command-invocation")
	echo poll >>"$STUB/polls.log"
	[ -n "${STUB_SSM_STUCK:-}" ] && echo Pending || echo Success ;;
*) echo "stub aws: unhandled $svc $op" >&2; exit 1 ;;
esac
STUBEOF
cat >"$BIN/curl" <<'CURLEOF'
#!/usr/bin/env bash
# The metrics store answers /api/v1/query from $STUB/metric.json, or fails when it is absent;
# /health is dumpstar's "ok" 200, or an edge 302 to an Access login when $STUB/edge-only exists.
case "$*" in
*/api/v1/query*) [ -f "$STUB/metric.json" ] && cat "$STUB/metric.json" || exit 22 ;;
*) if [ -f "$STUB/edge-only" ]; then printf '\n302\n'; else printf 'ok\n200\n'; fi ;;
esac
CURLEOF
printf '#!/usr/bin/env bash\nexit 0\n' >"$BIN/sleep"
printf '#!/usr/bin/env bash\necho "terraform was called: $*" >>"%s/terraform.called"; exit 1\n' "$WORK" >"$BIN/terraform"
chmod +x "$BIN"/*

export STUB PATH="$BIN:$PATH"
export DUMPSTAR_ENV_AWS_PROFILE=p DUMPSTAR_ENV_REGION=us-west-2 DUMPSTAR_ENV_AMI=ami-1 DUMPSTAR_ENV_SUBNET=subnet-1 \
	DUMPSTAR_ENV_SECURITY_GROUP=sg-1 DUMPSTAR_ENV_INSTANCE_TYPE=c7i.large DUMPSTAR_ENV_INSTANCE_PROFILE=envvm \
	DUMPSTAR_ENV_ACCESS_TEAM=team DUMPSTAR_ENV_CLOUDFLARED_IMAGE=docker.io/cloudflare/cloudflared:2026.1.0 \
	DUMPSTAR_ENV_OWNER=tester DUMPSTAR_ENV_BACKUP_BUCKET=bk DUMPSTAR_ENV_BACKUP_PATH=prod/tenants DUMPSTAR_ENV_BACKUP_REGION=us-west-2 \
	DUMPSTAR_ENV_CONCIERGE_DB=01KZHQVMMS4CSRASCMG5XRCPC3 DUMPSTAR_ENV_CONCIERGE_EMAIL=concierge@example.com \
	DUMPSTAR_ENV_CONCIERGE_SUB=78df0719-1f59-4ec3-b4f5-783908e37499 DUMPSTAR_ENV_CONCIERGE_ISSUER=https://id.example
ENV="$REPO_DIR/deploy/env.sh"
TREE=$(printf 'a%.0s' $(seq 40))
TREE2=$(printf 'b%.0s' $(seq 40))

out="$(bash "$ENV" up --image "tree:$TREE" 2>&1)" || { bad "up failed"; echo "$out"; }
grep -q 'url: https://env-1.dumpster.cards/' <<<"$out" && ok "up prints the slot URL" || { bad "up prints the slot URL"; echo "$out"; }
grep -q 'sign in: Pocket ID' <<<"$out" && ok "up says how to sign in" || bad "up says how to sign in"
grep -q 'edge share [0-9]*s' <<<"$out" && ok "up records the edge share" || bad "up records the edge share"
id1="$(basename "$(ls "$STUB"/instances/*.json | head -1)" .json)"
check "edge share is a tag on the VM" 1 "$(jq '.tags | has("dumpstar-env:edge-seconds")' "$STUB/instances/$id1.json" | grep -c true)"
check "the launch carries the pool tag the policy demands" round "$(jq -r '.tags["spill:owner"]' "$STUB/instances/$id1.json")"

# An edge redirect with no origin behind the tunnel is not dumpstar answering (sp-ycmkmh).
cp "$STUB/ledger" "$STUB/ledger.keep"; cp "$STUB/ssm.log" "$STUB/ssm.keep"
: >"$STUB/edge-only"
edge_out="$(bash "$ENV" up --image "tree:$TREE2" --name env-3 2>&1)" && bad "an edge 302 fails up" || ok "an edge 302 fails up"
grep -q 'HTTP 302' <<<"$edge_out" && ok "the failure names what answered" || bad "the failure names what answered"
rm -f "$STUB/edge-only"
cp "$STUB/ledger.keep" "$STUB/ledger"; cp "$STUB/ssm.keep" "$STUB/ssm.log"
check "the root volume holds the default floor" 60 "$(cat "$STUB/last-root-gb")"
STUB_AMI_SNAPSHOT_GB=64 bash "$ENV" up --image "tree:$TREE2" --name env-3 >/dev/null 2>&1 || bad "a 64 GB snapshot AMI launches"
check "the root volume is at least the AMI snapshot (64)" 64 "$(cat "$STUB/last-root-gb")"
bash "$ENV" down env-3 >/dev/null 2>&1 || true
n_before="$(ls "$STUB/instances" | wc -l)"
refused="$(STUB_AMI_SNAPSHOT_GB=64 DUMPSTAR_ENV_ROOT_GB=60 bash "$ENV" up --image "tree:$TREE2" --name env-3 2>&1)" && bad "a root smaller than the snapshot is refused" || ok "a root smaller than the snapshot is refused"
grep -q '60.*64' <<<"$refused" && ok "the refusal names both sizes" || bad "the refusal names both sizes"
check "the refusal precedes any create call" "$n_before" "$(ls "$STUB/instances" | wc -l)"
# The probe launch above is not part of the walk below: later counts read the ledger and ssm log.
cp "$STUB/ledger.keep" "$STUB/ledger"; cp "$STUB/ssm.keep" "$STUB/ssm.log"

out2="$(bash "$ENV" up --image "tree:$TREE" 2>&1)" && grep -q 'env-2.dumpster.cards' <<<"$out2" && ok "a second up takes the next slot" || bad "a second up takes the next slot"
bash "$ENV" up --image "tree:$TREE" --name env-1 >/dev/null 2>&1 && bad "a taken name is refused" || ok "a taken name is refused"
bash "$ENV" up --image "tree:$TREE" --name env-9 >/dev/null 2>&1 && bad "an unknown slot is refused" || ok "an unknown slot is refused"
bash "$ENV" up --image "tree:abc" >/dev/null 2>&1 && bad "a short tree is refused" || ok "a short tree is refused"
st="$(bash "$ENV" status)"; grep -qE "^env-1 +tester.*${TREE}" <<<"$st" && ok "status lists owner, build and URL" || bad "status lists owner, build and URL"
grep -q '^env-3 *free' <<<"$(bash "$ENV" status)" && ok "status shows the free slot" || bad "status shows the free slot"

bash "$ENV" update env-1 --image "tree:$TREE2" >/dev/null && ok "update succeeds" || bad "update succeeds"
check "update records the new build" "tree:$TREE2" "$(jq -r '.tags["dumpstar-env:build"]' "$STUB/instances/$id1.json")"

# 6. A ref or an unpushed tree is built locally, tagged -local, and prod refuses the tag (sp-b2pnhw).
printf '#!/usr/bin/env bash\necho "podman $*" >>"%s/podman.log"\nexit 0\n' "$WORK" >"$BIN/podman"; chmod +x "$BIN/podman"
DIRTY="$WORK/dirty"; mkdir -p "$DIRTY"
git -C "$DIRTY" init -q && printf 'FROM scratch\n' >"$DIRTY/Containerfile" && git -C "$DIRTY" add -A &&
	git -C "$DIRTY" -c user.name=t -c user.email=t@t commit -qm base
printf 'uncommitted\n' >"$DIRTY/new.txt"
dtree="$(git -C "$DIRTY" rev-parse 'HEAD^{tree}')"
bash "$ENV" update env-1 --tree "$DIRTY" >/dev/null 2>&1 && ok "update --tree a dirty tree succeeds" || bad "update --tree a dirty tree succeeds"
btag="$(jq -r '.tags["dumpstar-env:build"]' "$STUB/instances/$id1.json")"
case "$btag" in dir:*) ok "the dirty tree is recorded as a local build" ;; *) bad "the dirty tree is recorded as a local build ($btag)" ;; esac
lref="$(grep -o 'tree-[0-9a-f]\{40\}-local' "$WORK/podman.log" | head -1)"
[ -n "$lref" ] && ok "the image is tagged tree-<sha>-local" || bad "the image is tagged tree-<sha>-local"
[ "$lref" != "tree-${dtree}-local" ] && ok "the dirty file changes the tree id" || bad "the dirty file changes the tree id"
grep -q '^podman build' "$WORK/podman.log" && grep -q '^podman push .*-local' "$WORK/podman.log" && ok "built and published" || bad "built and published"
bash "$ENV" update env-1 --ref HEAD >/dev/null 2>&1 && ok "update --ref builds the ref" || bad "update --ref builds the ref"
bash "$ENV" update env-1 --ref no-such-ref >/dev/null 2>&1 && bad "an unknown ref is refused" || ok "an unknown ref is refused"
( . "$REPO_DIR/deploy/published-image-lib.sh"
  dumpstar_refuse_local_image prod "x:tree-${TREE}-local" 2>/dev/null && exit 1
  dumpstar_refuse_local_image prod "x:tree-${TREE}" && dumpstar_refuse_local_image test "x:tree-${TREE}-local" ) && ok "prod refuses a -local tag, CI's tag passes" || bad "prod refuses a -local tag, CI's tag passes"
grep -q 'dumpstar_refuse_local_image "\$INSTANCE" "\$PUBLISHED" "\${DUMPSTAR_PREBUILT_IMAGE:-}"' "$REPO_DIR/deploy/deploy.sh" && ok "deploy.sh runs the refusal before building" || bad "deploy.sh runs the refusal before building"
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

# 7. A failed up or a connector that will not stop never hangs and never leaves a VM (sp-s2zynj).
live() { cat "$STUB"/instances/*.json | jq -s '[.[] | select(.state != "terminated")] | length'; }
check "nothing is live before the failure paths" 0 "$(live)"
STUB_SSM_DENY=1 timeout 60 bash "$ENV" up --image "tree:$TREE" >"$WORK/deny.out" 2>&1 && bad "an up whose SendCommand is denied exits non-zero" || ok "an up whose SendCommand is denied exits non-zero"
check "the denied up terminated the instance it launched" 0 "$(live)"
bash "$ENV" up --image "tree:$TREE" >/dev/null
: >"$STUB/polls.log"
STUB_SSM_STUCK=1 timeout 60 bash "$ENV" down env-1 >"$WORK/stuck.out" 2>&1 && ok "down with a connector that will not stop succeeds" || { bad "down with a connector that will not stop succeeds"; cat "$WORK/stuck.out"; }
check "down terminated the VM despite the stuck connector" 0 "$(live)"
grep -q 'WARNING could not stop the connector' "$WORK/stuck.out" && ok "down names the connector warning" || bad "down names the connector warning"
[ "$(wc -l <"$STUB/polls.log")" -le 31 ] && ok "down stops polling at its deadline" || bad "down stops polling at its deadline ($(wc -l <"$STUB/polls.log") polls)"
bash "$ENV" up --image "tree:$TREE" >/dev/null
STUB_SSM_DENY=1 timeout 60 bash "$ENV" down env-1 >/dev/null 2>&1 && ok "down with a denied stop succeeds" || bad "down with a denied stop succeeds"
check "down with a denied stop leaves no VM" 0 "$(live)"

# 8. Every remote script parses under dash, which is what AWS-RunShellScript runs (sp-dt1lnh).
if command -v dash >/dev/null; then
	nscripts=0; badscripts=0
	while IFS= read -r line; do
		nscripts=$((nscripts + 1))
		jq -r '.Parameters.commands[0]' <<<"$line" >"$WORK/remote.sh"
		dash -n "$WORK/remote.sh" 2>"$WORK/dash.err" || { badscripts=$((badscripts + 1)); cat "$WORK/dash.err"; }
	done <"$STUB/ssm.log"
	[ "$nscripts" -gt 0 ] && [ "$badscripts" -eq 0 ] && ok "every remote script passes dash -n ($nscripts)" || bad "every remote script passes dash -n ($badscripts of $nscripts failed)"
else
	bad "dash is required to check the remote scripts"
fi

# 2. No Terraform.
[ ! -e "$WORK/terraform.called" ] && ok "no verb ran Terraform" || { bad "a verb ran Terraform"; cat "$WORK/terraform.called"; }

# 4. No secret in the shipped script; port is the edge's origin_service.
grep -q 'dumpstar/env-1/edge' "$STUB/ssm.log" && ok "the command names the secret" || bad "the command names the secret"
grep -qE "(TUNNEL_TOKEN|CLIENT_SECRET)=[A-Za-z0-9]{8,}" "$STUB/ssm.log" && bad "a secret value is in the SSM command" || ok "no secret value is in the SSM command"
want="$(sed -n 's|.*default *= *"http://127.0.0.1:\([0-9]*\)".*|\1|p' "$REPO_DIR/infra/edge/variables.tf")"
grep -q "export DUMPSTAR_ENV_PORT='${want}'" "$STUB/ssm.log" && ok "the origin port is infra/edge's origin_service" || bad "the origin port drifted from infra/edge"

# 6. The data: the concierge tenant's replica only, never the registry or another tenant.
script="$(jq -r '.Parameters.commands[0]' <<<"$(grep -m1 "DUMPSTAR_ENV_VERB='up'" "$STUB/ssm.log")")"
grep -q "export DUMPSTAR_ENV_CONCIERGE_REPLICA='s3://bk/prod/tenants/01KZHQVMMS4CSRASCMG5XRCPC3.sqlite?region=us-west-2'" <<<"$script" &&
	ok "up addresses the concierge tenant's replica" || bad "up addresses the concierge tenant's replica"
grep -q 'tenant create concierge' <<<"$script" && ! grep -qE 'tenant create (ryan|collection)|registry\.sqlite' <<<"$script" &&
	ok "the volume gets the concierge tenant and no other" || bad "the volume gets the concierge tenant and no other"
grep -q 'setup' <<<"$script" && ok "up builds the catalog" || bad "up builds the catalog"
(unset DUMPSTAR_ENV_CONCIERGE_DB; bash "$ENV" up --image "tree:$TREE" >/dev/null 2>&1) && bad "no concierge database id is refused" || ok "no concierge database id is refused"

# 9. A catalog-less slot is not ready (sp-0a1juj): env-vm.sh's catalog check against a stubbed podman.
grep -q 'catalog-apply --swu' <<<"$script" && ok "up builds the catalog with catalog-apply" || bad "up builds the catalog with catalog-apply"
printf '#!/usr/bin/env bash\necho "${STUB_CARDS:-0}"\n' >"$BIN/podman"; chmod +x "$BIN/podman"
catalog_verb() { DUMPSTAR_ENV_VERB=catalog DUMPSTAR_ENV_SLOT=env-1 bash "$REPO_DIR/deploy/env-vm.sh" 2>&1; }
cat_out="$(STUB_CARDS=0 catalog_verb)" && bad "an empty catalog fails up" || ok "an empty catalog fails up"
grep -q 'catalog is empty' <<<"$cat_out" && ok "the failure names the empty catalog" || bad "the failure names the empty catalog"
STUB_CARDS=1234 catalog_verb >/dev/null && ok "a catalog present passes" || bad "a catalog present passes"

# sp-hhx5dc: the concierge email is bound with its sub and issuer, so a bearer token resolves.
flat="$(tr '\n\\' '  ' <<<"$script")"
grep -qE 'tenant identity add concierge --email "\$DUMPSTAR_ENV_CONCIERGE_EMAIL"[[:space:]]+--sub "\$DUMPSTAR_ENV_CONCIERGE_SUB" --issuer "\$DUMPSTAR_ENV_CONCIERGE_ISSUER"' <<<"$flat" &&
	ok "up binds the concierge email with its sub and issuer" || bad "up binds the concierge email with its sub and issuer"
grep -q "export DUMPSTAR_ENV_CONCIERGE_SUB='78df0719-1f59-4ec3-b4f5-783908e37499'" <<<"$script" &&
	grep -q "export DUMPSTAR_ENV_CONCIERGE_ISSUER='https://id.example'" <<<"$script" &&
	ok "up ships the sub and issuer to the VM" || bad "up ships the sub and issuer to the VM"
n_before="$(ls "$STUB/instances" | wc -l)"
nosub="$(unset DUMPSTAR_ENV_CONCIERGE_SUB; bash "$ENV" up --image "tree:$TREE" 2>&1)" && bad "no concierge sub is refused" || ok "no concierge sub is refused"
grep -q 'DUMPSTAR_ENV_CONCIERGE_SUB' <<<"$nosub" && ok "the refusal names how to supply the sub" || bad "the refusal names how to supply the sub"
check "the sub refusal precedes any create call" "$n_before" "$(ls "$STUB/instances" | wc -l)"
(unset DUMPSTAR_ENV_CONCIERGE_ISSUER; bash "$ENV" up --image "tree:$TREE" >/dev/null 2>&1) && bad "no concierge issuer is refused" || ok "no concierge issuer is refused"

# 5. No fallback.
(unset DUMPSTAR_ENV_AMI; bash "$ENV" status >/dev/null 2>&1) && bad "a missing value is refused" || ok "a missing value is refused"

# 6. The reaper.
export DUMPSTAR_ENV_METRICS_URL=http://metrics.invalid:8428
bash "$ENV" up --image "tree:$TREE" >/dev/null
rid="$(basename "$(ls -t "$STUB"/instances/*.json | head -1)" .json)"
rjson="$STUB/instances/$rid.json"
launch="$(date -d "$(jq -r .launch "$rjson")" +%s)"
settag() { jq --arg v "$1" '.tags["dumpstar-env:last-activity"] = $v' "$rjson" >"$STUB/t" && mv "$STUB/t" "$rjson"; }
state() { jq -r .state "$rjson"; }
reap_at() { DUMPSTAR_ENV_NOW=$((launch + $1)) bash "$ENV" reap 2>&1; }
echo '{"status":"success","data":{"result":[]}}' >"$STUB/metric.json"
settag "$((launch + 60))"
reap_at 1800 >/dev/null; check "idle under 30 min survives" running "$(state)"
reap_at 1859 >/dev/null; check "idle 29m59s survives" running "$(state)"
echo '{"status":"success","data":{"result":[{"value":[0,"1"]}]}}' >"$STUB/metric.json"
reap_at 1900 >/dev/null; check "in use: survives past the idle limit" running "$(state)"
check "activity moves the idle clock" "$((launch + 1900))" "$(jq -r '.tags["dumpstar-env:last-activity"]' "$rjson")"
reap_at 3600 >/dev/null; check "in use: still alive at 1 h" running "$(state)"
echo '{"status":"success","data":{"result":[]}}' >"$STUB/metric.json"
reap_at $((3600 + 1800)) >/dev/null; check "idle for 30 min: torn down" terminated "$(state)"
check "the idle teardown leaves no volume" 0 "$(ls "$STUB/volumes" | wc -l)"

bash "$ENV" up --image "tree:$TREE" >/dev/null
rid="$(basename "$(ls -t "$STUB"/instances/*.json | head -1)" .json)"; rjson="$STUB/instances/$rid.json"
launch="$(date -d "$(jq -r .launch "$rjson")" +%s)"
echo '{"status":"success","data":{"result":[{"value":[0,"1"]}]}}' >"$STUB/metric.json"
settag "$((launch + 7000))"
reap_at 7199 >/dev/null; check "in use just under the TTL survives" running "$(state)"
reap_at 7200 >/dev/null; check "the TTL ends a busy environment" terminated "$(state)"

bash "$ENV" up --image "tree:$TREE" >/dev/null
rid="$(basename "$(ls -t "$STUB"/instances/*.json | head -1)" .json)"; rjson="$STUB/instances/$rid.json"
launch="$(date -d "$(jq -r .launch "$rjson")" +%s)"
rm -f "$STUB/metric.json"
settag "$((launch + 60))"
out="$(reap_at 5000)"; check "unreadable counter: not idled out" running "$(state)"
grep -q 'activity unknown' <<<"$out" && ok "the reaper says activity is unknown" || bad "the reaper says activity is unknown"
grep -qE '^env-1 .*activity unknown' <<<"$(bash "$ENV" status)" && ok "status shows activity unknown" || bad "status shows activity unknown"
echo '{"status":"error","data":{}}' >"$STUB/metric.json"
reap_at 5000 >/dev/null; check "an error answer is unknown too" running "$(state)"
check "unknown never moves the idle clock" "$((launch + 60))" "$(jq -r '.tags["dumpstar-env:last-activity"]' "$rjson")"
reap_at 7200 >/dev/null; check "unreadable counter: the TTL still applies" terminated "$(state)"
(unset DUMPSTAR_ENV_METRICS_URL; bash "$ENV" up --image "tree:$TREE" >/dev/null
	rid="$(basename "$(ls -t "$STUB"/instances/*.json | head -1)" .json)"
	launch="$(date -d "$(jq -r .launch "$STUB/instances/$rid.json")" +%s)"
	DUMPSTAR_ENV_NOW=$((launch + 7300)) bash "$ENV" reap >/dev/null 2>&1
	[ "$(jq -r .state "$STUB/instances/$rid.json")" = terminated ]) && ok "no metrics URL: TTL only" || bad "no metrics URL: TTL only"

echo "env_test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
