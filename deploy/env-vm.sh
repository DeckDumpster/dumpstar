#!/usr/bin/env bash
#
# What runs ON an environment's VM (sp-8i63uo). deploy/env.sh ships this file over SSM
# after a header that exports the parameters and unpacks the rendered Quadlets and env files
# into $DUMPSTAR_ENV_RENDER; nothing here is run from a checkout, and nothing here runs
# Terraform. DUMPSTAR_ENV_VERB selects the step:
#
#   up      write the slot's env files (secrets from Secrets Manager, never echoed), install
#           the units, pull the image, start the container and the tunnel connector
#   update  pull a different build, tag it as the slot's image, restart the container only:
#           hostname, client, data and connector are untouched
#   stop    stop the tunnel connector and the container (before the VM is destroyed)
#
# Rootful podman, system-level Quadlets: an environment VM has no login user to linger.
set -euo pipefail

: "${DUMPSTAR_ENV_VERB:?}" "${DUMPSTAR_ENV_SLOT:?}"
slot="$DUMPSTAR_ENV_SLOT"
secrets_dir="/etc/dumpstar/${slot}"
unit_dir="/etc/containers/systemd"
app="dumpstar-${slot}"
conn="dumpstar-cloudflared-${slot}"

pull_and_tag() {
	podman pull --quiet "$DUMPSTAR_ENV_IMAGE_REF" >/dev/null
	podman tag "$DUMPSTAR_ENV_IMAGE_REF" "localhost/dumpstar:${slot}"
}

# set_key <file> <KEY> <value>: replace the (possibly commented-out) KEY= line, keep the rest.
set_key() {
	local file="$1" key="$2" val="$3" tmp
	tmp="$(mktemp)"
	awk -v k="$key" -v v="$val" 'BEGIN { re = "^#?" k "=" }
		$0 ~ re { if (!done) print k "=" v; done = 1; next } { print }
		END { if (!done) print k "=" v }' "$file" >"$tmp"
	cat "$tmp" >"$file"
	rm -f "$tmp"
}

wait_health() {
	local i
	for i in $(seq 1 60); do
		curl -fsS -o /dev/null "http://127.0.0.1:${DUMPSTAR_ENV_PORT}/health" && return 0
		sleep 1
	done
	echo "env-vm: ${app} did not answer /health in 60s" >&2
	return 1
}

case "$DUMPSTAR_ENV_VERB" in
up)
	: "${DUMPSTAR_ENV_RENDER:?}" "${DUMPSTAR_ENV_IMAGE_REF:?}" "${DUMPSTAR_ENV_REGION:?}" \
		"${DUMPSTAR_ENV_ACCESS_TEAM:?}" "${DUMPSTAR_ENV_HOSTNAME:?}" "${DUMPSTAR_ENV_PORT:?}" \
		"${DUMPSTAR_ENV_EDGE_SECRET:?}" "${DUMPSTAR_ENV_CLOUDFLARED_IMAGE:?}"
	install -d -m 0700 "$secrets_dir" "$secrets_dir/aws"
	install -d -m 0755 "$unit_dir"
	cp "$DUMPSTAR_ENV_RENDER"/*.env "$secrets_dir/"
	set +x
	edge="$(aws --region "$DUMPSTAR_ENV_REGION" secretsmanager get-secret-value \
		--secret-id "$DUMPSTAR_ENV_EDGE_SECRET" --query SecretString --output text)"
	val() { jq -r --arg k "$1" '.[$k] // empty' <<<"$edge"; }
	for k in CLOUDFLARED_TUNNEL_TOKEN DUMPSTAR_OIDC_CLIENT_ID DUMPSTAR_OIDC_CLIENT_SECRET DUMPSTAR_ACCESS_AUD; do
		[ -n "$(val "$k")" ] || { echo "env-vm: ${DUMPSTAR_ENV_EDGE_SECRET} has no ${k}" >&2; exit 1; }
	done
	team="https://${DUMPSTAR_ENV_ACCESS_TEAM}.cloudflareaccess.com"
	set_key "$secrets_dir/access.env" DUMPSTAR_ACCESS_TEAM_DOMAIN "$team"
	set_key "$secrets_dir/access.env" DUMPSTAR_ACCESS_AUD "$(val DUMPSTAR_ACCESS_AUD)"
	set_key "$secrets_dir/access.env" DUMPSTAR_ACCESS_JWKS_URL "${team}/cdn-cgi/access/certs"
	set_key "$secrets_dir/oidc.env" DUMPSTAR_OIDC_ISSUER "https://id.gantt.pub"
	set_key "$secrets_dir/oidc.env" DUMPSTAR_OIDC_CLIENT_ID "$(val DUMPSTAR_OIDC_CLIENT_ID)"
	set_key "$secrets_dir/oidc.env" DUMPSTAR_OIDC_CLIENT_SECRET "$(val DUMPSTAR_OIDC_CLIENT_SECRET)"
	set_key "$secrets_dir/oidc.env" DUMPSTAR_OIDC_REDIRECT_URL "https://${DUMPSTAR_ENV_HOSTNAME}/auth/callback"
	set_key "$secrets_dir/oidc.env" DUMPSTAR_OIDC_REQUIRED_GROUP "dumpstar-users"
	set_key "$secrets_dir/oidc.env" DUMPSTAR_OIDC_COOKIE_KEY "$(openssl rand -hex 32)"
	( umask 077; printf 'TUNNEL_TOKEN=%s\n' "$(val CLOUDFLARED_TUNNEL_TOKEN)" >"$secrets_dir/cloudflared.env" )
	chmod 0600 "$secrets_dir"/*.env
	unset edge

	# The tunnel connector: host networking so 127.0.0.1:<port> is the app's published port.
	cat >"$unit_dir/${conn}.container" <<UNIT
[Unit]
Description=Cloudflare tunnel connector (${slot})
After=${app}.service network-online.target
Wants=network-online.target

[Container]
Image=${DUMPSTAR_ENV_CLOUDFLARED_IMAGE}
Exec=tunnel --no-autoupdate run
EnvironmentFile=${secrets_dir}/cloudflared.env
Network=host

[Service]
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
	cp "$DUMPSTAR_ENV_RENDER/dumpstar-${slot}.container" "$unit_dir/${app}.container"
	pull_and_tag
	systemctl daemon-reload
	systemctl start "${app}.service"
	wait_health
	systemctl start "${conn}.service"
	echo "env-vm: connector started"
	;;
update)
	: "${DUMPSTAR_ENV_IMAGE_REF:?}" "${DUMPSTAR_ENV_PORT:?}"
	pull_and_tag
	systemctl restart "${app}.service"
	wait_health
	;;
stop)
	systemctl stop "${conn}.service" 2>/dev/null || true
	systemctl stop "${app}.service" 2>/dev/null || true
	;;
*)
	echo "env-vm: unknown verb ${DUMPSTAR_ENV_VERB}" >&2
	exit 2
	;;
esac
