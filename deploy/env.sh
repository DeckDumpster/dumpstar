#!/usr/bin/env bash
#
# env — disposable dumpstar environments on an EC2 VM (sp-8i63uo; design "Disposable test
# environments from any build", 2026-10-21).
#
#   bash deploy/env.sh up --image tree:<sha> [--name env-N]
#   bash deploy/env.sh update env-N --image tree:<sha>
#   bash deploy/env.sh status
#   bash deploy/env.sh down env-N
#
# A slot is env-1..env-3. Its hostname, tunnel, Access application and Pocket ID client were
# pre-provisioned once, in the slot's own Terraform workspace (infra/edge), so NOTHING HERE RUNS
# TERRAFORM: up launches a VM and starts the slot's container and tunnel connector on it, update
# swaps the container, down destroys the VM. The slot's edge outlives every environment.
#
# The registry is the VM's tags: a slot is taken while an instance tagged
# dumpstar-env:slot=<slot> exists in any state but terminated. Owner, build, last activity and the
# edge share of up are tags too, so `status` is one describe-instances.
#
# Every resource `up` creates is a class named in DUMPSTAR_ENV_RESOURCES; `down` removes each, and
# tests/deploy/env_test.sh fails when a class is created that the list does not name.
#
# Required configuration (environment or the file named by DUMPSTAR_ENV_CONF); there is no default
# for any of it, and a missing value refuses the command:
#   DUMPSTAR_ENV_AWS_PROFILE  the profile that may launch and terminate tagged instances
#   DUMPSTAR_ENV_REGION  DUMPSTAR_ENV_AMI  DUMPSTAR_ENV_SUBNET  DUMPSTAR_ENV_SECURITY_GROUP
#   DUMPSTAR_ENV_INSTANCE_TYPE  DUMPSTAR_ENV_INSTANCE_PROFILE
#       the instance profile lets the VM read secret dumpstar/<slot>/edge and nothing else
#   DUMPSTAR_ENV_ACCESS_TEAM  the Zero Trust team name (infra/edge access_team_name)
#   DUMPSTAR_ENV_CLOUDFLARED_IMAGE  the pinned connector image
#   DUMPSTAR_ENV_BACKUP_BUCKET  DUMPSTAR_ENV_BACKUP_PATH  DUMPSTAR_ENV_BACKUP_REGION
#       prod's Litestream target (LITESTREAM_S3_BUCKET / _PATH / _REGION in its litestream.env);
#       the instance profile may read the concierge tenant's replica under it and nothing else
#   DUMPSTAR_ENV_CONCIERGE_DB  the concierge tenant's database id (dumpstar tenant list on prod)
#   DUMPSTAR_ENV_CONCIERGE_EMAIL  the verified email bound to the concierge tenant in the environment
#
# THE DATA (sp-2xkn6g): up restores the concierge tenant's latest Litestream replica into the
# slot's volume, as the only tenant of a fresh registry, beside a freshly built catalog. Never
# Ryan's tenant, never prod's live database, nothing synced afterwards; drift is accepted.
# The AMI must carry podman (Quadlet), jq, curl, openssl, the aws CLI and the SSM agent.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/alert-gate.sh
. "$SCRIPT_DIR/alert-gate.sh"
# shellcheck source=deploy/render-lib.sh
. "$SCRIPT_DIR/render-lib.sh"
# shellcheck source=deploy/published-image-lib.sh
. "$SCRIPT_DIR/published-image-lib.sh"
# shellcheck source=deploy/litestream-lib.sh
. "$SCRIPT_DIR/litestream-lib.sh"

DUMPSTAR_ENV_SLOTS="env-1 env-2 env-3"
# What the slot's tunnel forwards to: the origin_service default in infra/edge/variables.tf, read, not copied.
DUMPSTAR_ENV_PORT="$(sed -n 's|.*default *= *"http://127.0.0.1:\([0-9]*\)".*|\1|p' "$SCRIPT_DIR/../infra/edge/variables.tf" | head -1)"
[ -n "$DUMPSTAR_ENV_PORT" ] || { echo "env: no origin_service default in infra/edge/variables.tf" >&2; exit 1; }
# Budgets, in seconds, from the design: the whole of up, the real edge's share of it, and update.
DUMPSTAR_ENV_BUDGET_UP=300
DUMPSTAR_ENV_BUDGET_EDGE=120
DUMPSTAR_ENV_BUDGET_UPDATE=120
# Resource classes `up` creates, each removed by `down`.
DUMPSTAR_ENV_RESOURCES="instance volume data connector"
# The tag every launch must carry: the launch policy allows only instances so tagged.
DUMPSTAR_ENV_POOL_TAG="spill:owner=round"

die() { echo "env: $*" >&2; exit 1; }
now() { date +%s; }

env_load_config() {
	if [ -n "${DUMPSTAR_ENV_CONF:-}" ]; then
		[ -f "$DUMPSTAR_ENV_CONF" ] || die "DUMPSTAR_ENV_CONF=$DUMPSTAR_ENV_CONF does not exist"
		# shellcheck disable=SC1090
		. "$DUMPSTAR_ENV_CONF"
	fi
	local v name
	for v in AWS_PROFILE REGION AMI SUBNET SECURITY_GROUP INSTANCE_TYPE INSTANCE_PROFILE ACCESS_TEAM CLOUDFLARED_IMAGE BACKUP_BUCKET BACKUP_PATH BACKUP_REGION CONCIERGE_DB CONCIERGE_EMAIL; do
		name="DUMPSTAR_ENV_$v"
		[ -n "${!name:-}" ] || die "$name is not set (no default; see the header of deploy/env.sh)"
	done
}

aws_() { aws --profile "$DUMPSTAR_ENV_AWS_PROFILE" --region "$DUMPSTAR_ENV_REGION" "$@"; }

valid_slot() {
	local s
	for s in $DUMPSTAR_ENV_SLOTS; do [ "$1" = "$s" ] && return 0; done
	return 1
}

# parse_build <spec>: sets IMAGE_REF. Only a CI image by tree is built here; a ref or a tree is
# sp-b2pnhw.
parse_build() {
	case "$1" in
	tree:*)
		local sha="${1#tree:}"
		[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || die "tree:<sha> needs the 40-hex tree id, got '$sha'"
		IMAGE_REF="${DUMPSTAR_IMAGE_REPO}:tree-${sha}" ;;
	*) die "unsupported build '$1': only --image tree:<sha> (a ref or an unpushed tree is sp-b2pnhw)" ;;
	esac
}

# slot_instances <slot>: JSON lines {id,state,launch,tags} of the live instances holding the slot.
slot_instances() {
	aws_ ec2 describe-instances \
		--filters "Name=tag:dumpstar-env:slot,Values=$1" \
		"Name=instance-state-name,Values=pending,running,stopping,stopped,shutting-down" |
		jq -c '[.Reservations[].Instances[] | {id: .InstanceId, state: .State.Name, launch: .LaunchTime,
			tags: ((.Tags // []) | map({(.Key): .Value}) | add // {})}] | sort_by(.launch, .id) | .[]'
}

tag_instance() { # tag_instance <id> Key=Value...
	local id="$1" t args=()
	shift
	for t in "$@"; do args+=("Key=${t%%=*},Value=${t#*=}"); done
	aws_ ec2 create-tags --resources "$id" --tags "${args[@]}"
}

# ssm_run <instance-id> <verb> [KEY=VALUE ...]: run env-vm.sh there with the render bundle; waits.
ssm_run() {
	local id="$1" verb="$2" slot="$3" render="$4" kv script cmd cid st i
	shift 4
	script="set -euo pipefail"$'\n'
	for kv in "DUMPSTAR_ENV_VERB=$verb" "DUMPSTAR_ENV_SLOT=$slot" "DUMPSTAR_ENV_PORT=$DUMPSTAR_ENV_PORT" \
		"DUMPSTAR_ENV_REGION=$DUMPSTAR_ENV_REGION" "$@"; do
		script+="export ${kv%%=*}='${kv#*=}'"$'\n'
	done
	script+="mkdir -p /tmp/dumpstar-env-render && echo '$(tar -C "$render" -czf - . | base64 -w0)' | base64 -d | tar -xzf - -C /tmp/dumpstar-env-render"$'\n'
	script+="export DUMPSTAR_ENV_RENDER=/tmp/dumpstar-env-render"$'\n'
	script+="$(cat "$SCRIPT_DIR/env-vm.sh")"
	cmd="$(jq -n --arg s "$script" --arg id "$id" \
		'{InstanceIds: [$id], DocumentName: "AWS-RunShellScript", TimeoutSeconds: 600, Parameters: {commands: [$s]}}')"
	cid="$(aws_ ssm send-command --cli-input-json "$cmd" --query Command.CommandId --output text)"
	for i in $(seq 1 300); do
		st="$(aws_ ssm get-command-invocation --command-id "$cid" --instance-id "$id" --query Status --output text 2>/dev/null || echo Pending)"
		case "$st" in
		Success) return 0 ;;
		Failed | Cancelled | TimedOut)
			aws_ ssm get-command-invocation --command-id "$cid" --instance-id "$id" \
				--query StandardErrorContent --output text >&2 || true
			return 1 ;;
		esac
		sleep 2
	done
	return 1
}

wait_ssm() {
	local id="$1" i
	for i in $(seq 1 90); do
		[ "$(aws_ ssm describe-instance-information --filters "Key=InstanceIds,Values=$id" \
			--query 'InstanceInformationList[0].PingStatus' --output text 2>/dev/null)" = Online ] && return 0
		sleep 2
	done
	return 1
}

# wait_edge <hostname>: seconds until Cloudflare's edge answers for the hostname with anything but
# an unreachable-origin or tunnel-down status. The User-Agent is set: the zone refuses none (1010).
wait_edge() {
	local code i
	for i in $(seq 1 240); do
		code="$(curl -s -o /dev/null -w '%{http_code}' -A "dumpstar-env/1" --max-time 5 "https://$1/health" || true)"
		case "$code" in 200 | 301 | 302 | 303 | 307 | 401 | 403) return 0 ;; esac
		sleep 1
	done
	return 1
}

# concierge_replica_url: the latest replica of the concierge tenant, addressed exactly as prod's
# sidecar wrote it (litestream-lib.sh). A database id that is not one is refused there.
concierge_replica_url() {
	validate_database_id "$DUMPSTAR_ENV_CONCIERGE_DB" >/dev/null || die "DUMPSTAR_ENV_CONCIERGE_DB is not a database id"
	LITESTREAM_S3_BUCKET="$DUMPSTAR_ENV_BACKUP_BUCKET" LITESTREAM_S3_PATH="$DUMPSTAR_ENV_BACKUP_PATH" \
		LITESTREAM_S3_REGION="$DUMPSTAR_ENV_BACKUP_REGION" tenant_replica_url "$DUMPSTAR_ENV_CONCIERGE_DB"
}

render_slot() { # render_slot <slot> <outdir>
	bash "$SCRIPT_DIR/render.sh" "$2" "$1" --hostname "$(hostname_of "$1")" \
		--port "$DUMPSTAR_ENV_PORT" --secrets "/etc/dumpstar/$1"
}

hostname_of() { printf '%s.test.dumpster.cards' "$1"; }

cmd_up() {
	local image="" slot="" t0 t1 t2 id s tmp mine first
	while [ $# -gt 0 ]; do
		case "$1" in
		--image) image="${2:-}"; shift 2 ;;
		--name) slot="${2:-}"; shift 2 ;;
		*) die "up: unknown argument $1" ;;
		esac
	done
	[ -n "$image" ] || die "up: --image tree:<sha> is required"
	parse_build "$image"
	env_load_config
	t0="$(now)"
	if [ -n "$slot" ]; then
		valid_slot "$slot" || die "up: $slot is not one of: $DUMPSTAR_ENV_SLOTS"
		[ -z "$(slot_instances "$slot")" ] || die "up: $slot is taken (env status)"
	else
		for s in $DUMPSTAR_ENV_SLOTS; do
			[ -z "$(slot_instances "$s")" ] && { slot="$s"; break; }
		done
		[ -n "$slot" ] || die "up: all slots are taken ($DUMPSTAR_ENV_SLOTS); env down one"
	fi
	tmp="$(mktemp -d)"
	ENV_TMP="$tmp"
	render_slot "$slot" "$tmp/render"

	local owner="${DUMPSTAR_ENV_OWNER:-${USER:-unknown}}"
	local tags="{Key=${DUMPSTAR_ENV_POOL_TAG%%=*},Value=${DUMPSTAR_ENV_POOL_TAG#*=}},{Key=Name,Value=dumpstar-$slot},{Key=dumpstar-env:slot,Value=$slot},{Key=dumpstar-env:owner,Value=$owner},{Key=dumpstar-env:build,Value=${image}},{Key=dumpstar-env:last-activity,Value=$t0}"
	id="$(aws_ ec2 run-instances --image-id "$DUMPSTAR_ENV_AMI" --instance-type "$DUMPSTAR_ENV_INSTANCE_TYPE" \
		--subnet-id "$DUMPSTAR_ENV_SUBNET" --security-group-ids "$DUMPSTAR_ENV_SECURITY_GROUP" \
		--iam-instance-profile "Name=$DUMPSTAR_ENV_INSTANCE_PROFILE" --count 1 \
		--block-device-mappings 'DeviceName=/dev/sda1,Ebs={VolumeSize=60,VolumeType=gp3,DeleteOnTermination=true}' \
		--tag-specifications "ResourceType=instance,Tags=[$tags]" "ResourceType=volume,Tags=[$tags]" \
		--query 'Instances[0].InstanceId' --output text)"
	# Two ups can pick the same slot: the earlier launch keeps it, the later gives itself back.
	first="$(slot_instances "$slot" | head -1 | jq -r .id)"
	if [ "$first" != "$id" ]; then
		aws_ ec2 terminate-instances --instance-ids "$id" >/dev/null
		die "up: lost the race for $slot to $first; run up again"
	fi
	wait_ssm "$id" || { cmd_down_id "$id"; die "up: $id never reported to SSM; destroyed"; }
	ssm_run "$id" up "$slot" "$tmp/render" "DUMPSTAR_ENV_IMAGE_REF=$IMAGE_REF" \
		"DUMPSTAR_ENV_ACCESS_TEAM=$DUMPSTAR_ENV_ACCESS_TEAM" "DUMPSTAR_ENV_HOSTNAME=$(hostname_of "$slot")" \
		"DUMPSTAR_ENV_EDGE_SECRET=dumpstar/$slot/edge" "DUMPSTAR_ENV_CLOUDFLARED_IMAGE=$DUMPSTAR_ENV_CLOUDFLARED_IMAGE" \
		"DUMPSTAR_ENV_CONCIERGE_REPLICA=$(concierge_replica_url)" "DUMPSTAR_ENV_LITESTREAM_IMAGE=$DUMPSTAR_LITESTREAM_IMAGE" \
		"DUMPSTAR_ENV_CONCIERGE_EMAIL=$DUMPSTAR_ENV_CONCIERGE_EMAIL" ||
		{ cmd_down_id "$id"; die "up: starting $slot failed; the VM was destroyed"; }
	t1="$(now)"
	wait_edge "$(hostname_of "$slot")" || { cmd_down_id "$id"; die "up: the edge never answered for $(hostname_of "$slot"); the VM was destroyed"; }
	t2="$(now)"
	tag_instance "$id" "dumpstar-env:edge-seconds=$((t2 - t1))" "dumpstar-env:up-seconds=$((t2 - t0))" "dumpstar-env:last-activity=$t2"
	echo "env: $slot up in $((t2 - t0))s (edge share $((t2 - t1))s, instance $id)"
	[ $((t2 - t1)) -le "$DUMPSTAR_ENV_BUDGET_EDGE" ] || echo "env: WARNING edge share over the ${DUMPSTAR_ENV_BUDGET_EDGE}s budget; the design's fallback is tailnet-only" >&2
	[ $((t2 - t0)) -le "$DUMPSTAR_ENV_BUDGET_UP" ] || echo "env: WARNING up over the ${DUMPSTAR_ENV_BUDGET_UP}s budget" >&2
	echo "url: https://$(hostname_of "$slot")/"
	echo "sign in: Pocket ID (https://id.gantt.pub) as a member of the dumpstar-users group; the MCP audience is https://$(hostname_of "$slot")/mcp"
}

cmd_update() {
	local slot="${1:-}" image="" t0 id tmp
	[ -n "$slot" ] || die "update: env-N is required"
	shift
	while [ $# -gt 0 ]; do
		case "$1" in
		--image) image="${2:-}"; shift 2 ;;
		--ref | --tree) die "update: $1 is not built yet (sp-b2pnhw); use --image tree:<sha>" ;;
		*) die "update: unknown argument $1" ;;
		esac
	done
	[ -n "$image" ] || die "update: --image tree:<sha> is required"
	valid_slot "$slot" || die "update: $slot is not one of: $DUMPSTAR_ENV_SLOTS"
	parse_build "$image"
	env_load_config
	id="$(slot_instances "$slot" | head -1 | jq -r '.id // empty')"
	[ -n "$id" ] || die "update: $slot has no environment"
	t0="$(now)"
	tmp="$(mktemp -d)"
	ENV_TMP="$tmp"
	mkdir -p "$tmp/render"
	ssm_run "$id" update "$slot" "$tmp/render" "DUMPSTAR_ENV_IMAGE_REF=$IMAGE_REF" ||
		die "update: swapping the build on $slot failed; it still runs whatever it ran"
	tag_instance "$id" "dumpstar-env:build=$image" "dumpstar-env:last-activity=$(now)"
	echo "env: $slot updated in $(($(now) - t0))s to $image"
	[ $(($(now) - t0)) -le "$DUMPSTAR_ENV_BUDGET_UPDATE" ] || echo "env: WARNING update over the ${DUMPSTAR_ENV_BUDGET_UPDATE}s budget" >&2
}

cmd_status() {
	env_load_config
	local s line age la
	printf '%-6s %-9s %-14s %-48s %-6s %-10s %s\n' SLOT OWNER STATE BUILD AGE ACTIVITY URL
	for s in $DUMPSTAR_ENV_SLOTS; do
		line="$(slot_instances "$s" | head -1)"
		if [ -z "$line" ]; then
			printf '%-6s free\n' "$s"
			continue
		fi
		age="$(jq -r '.launch' <<<"$line")"
		age=$(($(now) - $(date -d "$age" +%s)))
		la="$(jq -r '.tags["dumpstar-env:last-activity"] // empty' <<<"$line")"
		if [ -n "$la" ]; then la="$(($(now) - la))s ago"; else la="unknown"; fi
		printf '%-6s %-9s %-14s %-48s %-6s %-10s %s\n' "$s" \
			"$(jq -r '.tags["dumpstar-env:owner"] // "?"' <<<"$line")" "$(jq -r .state <<<"$line")" \
			"$(jq -r '.tags["dumpstar-env:build"] // "?"' <<<"$line")" "${age}s" "$la" "https://$(hostname_of "$s")/"
	done
}

# cmd_down_id <instance-id>: stop the connector, destroy the VM and its volumes, wait for it.
cmd_down_id() {
	local id="$1" slot tmp vols v i st
	slot="$(aws_ ec2 describe-instances --instance-ids "$id" \
		--query 'Reservations[0].Instances[0].Tags[?Key==`dumpstar-env:slot`].Value | [0]' --output text)"
	if [ -n "$slot" ] && [ "$slot" != None ]; then
		tmp="$(mktemp -d)"
		mkdir -p "$tmp/render"
		# Best effort: a VM that cannot be reached loses its connector when it is terminated.
		ssm_run "$id" stop "$slot" "$tmp/render" || echo "env: could not stop the connector over SSM; terminating the VM ends it" >&2
		rm -rf "$tmp"
	fi
	aws_ ec2 terminate-instances --instance-ids "$id" >/dev/null
	for i in $(seq 1 90); do
		st="$(aws_ ec2 describe-instances --instance-ids "$id" --query 'Reservations[0].Instances[0].State.Name' --output text)"
		[ "$st" = terminated ] && break
		sleep 2
	done
	[ "$st" = terminated ] || die "down: $id did not reach terminated"
	# The root volume goes with the VM; any volume still tagged for the instance is deleted by hand.
	vols="$(aws_ ec2 describe-volumes --filters "Name=tag:dumpstar-env:slot,Values=${slot:-none}" \
		--query 'Volumes[].VolumeId' --output text)"
	for v in $vols; do aws_ ec2 delete-volume --volume-id "$v" >/dev/null; done
}

cmd_down() {
	local slot="${1:-}" id n=0
	[ -n "$slot" ] || die "down: env-N is required"
	valid_slot "$slot" || die "down: $slot is not one of: $DUMPSTAR_ENV_SLOTS"
	env_load_config
	while read -r id; do
		[ -n "$id" ] || continue
		cmd_down_id "$id"
		n=$((n + 1))
	done < <(slot_instances "$slot" | jq -r .id)
	[ "$n" -gt 0 ] || die "down: $slot has no environment"
	echo "env: $slot down; no VM and no volume left (the slot's Terraform is untouched)"
}

trap 'rm -rf "${ENV_TMP:-}"' EXIT
verb="${1:-}"
[ $# -gt 0 ] && shift
case "$verb" in
up) cmd_up "$@" ;;
update) cmd_update "$@" ;;
status) cmd_status ;;
down) cmd_down "$@" ;;
*) sed -n '3,9p' "$0" >&2; exit 2 ;;
esac
