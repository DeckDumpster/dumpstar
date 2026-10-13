#!/usr/bin/env bash
# Scaffold per-instance config files the Quadlet units require.
# Every function here is idempotent: it writes only when the file is absent,
# so it is safe to call on every setup AND every deploy (db-g6ku).

# Write the Cloudflare Access config file for <instance> into <conf_dir> if it
# does not already exist. Access is mandatory: the server refuses to start until
# the three values are filled in, and there is no mode that skips it.
# The Quadlet unit loads this with 'EnvironmentFile=...' (no leading '-', which
# Quadlet does not support), so the file must always exist before the unit
# starts.
#
# Usage: dumpstar_scaffold_access_env <instance> <conf_dir>
dumpstar_scaffold_access_env() {
    local instance="$1" conf_dir="$2"
    if [ ! -f "${conf_dir}/access.env" ]; then
        mkdir -p "$conf_dir"
        cat > "${conf_dir}/access.env" <<EOF
# Cloudflare Access config for instance '${instance}'.
# All three values are REQUIRED: dumpstar refuses to start without them. There
# is no single-tenant mode and no switch that turns authentication off.
#
# Steps:
# 1. In the Cloudflare Zero Trust dashboard, open Access > Applications.
# 2. Find the Dumpstar application and copy its AUD tag (64-char hex).
# 3. Uncomment and fill in the three values below.
# 4. Redeploy to pick up the new file: bash deploy/deploy.sh ${instance}
#
# Note: systemd EnvironmentFile does not strip trailing # comments from value
# lines. Put comments on their own lines, as shown here.
# Team domain — e.g. https://myteam.cloudflareaccess.com
#DUMPSTAR_ACCESS_TEAM_DOMAIN=CHANGE_ME
# 64-char hex AUD tag from Access > Applications
#DUMPSTAR_ACCESS_AUD=CHANGE_ME
# JWKS URL — e.g. https://myteam.cloudflareaccess.com/cdn-cgi/access/certs
#DUMPSTAR_ACCESS_JWKS_URL=CHANGE_ME
EOF
        chmod 600 "${conf_dir}/access.env"
        echo "    Wrote ${conf_dir}/access.env — fill the CHANGE_ME values; dumpstar will not start without them."
        echo "    See deploy/TENANTS.md §\"Configuring Access for a deployment\" for instructions."
    fi
}
