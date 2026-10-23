#!/usr/bin/env bash
# Registers (ensure) or removes (remove) one Pocket ID API resource: bash pocketid-api.sh <ensure|remove> <base url> <audience> <name>
# Pocket ID accepts an OAuth `resource` only if it is a registered API, and the Trozz/pocketid
# provider has no resource type for one, so main.tf runs this from a terraform_data.
# ensure adopts an API already registered for the audience (prod's hand-made one) and otherwise
# creates it; either way CIMD clients are allowed, which is what lets Claude Code sign in.
# POCKETID_API_TOKEN is the admin API key, from the environment.
set -euo pipefail
verb="${1:?usage: pocketid-api.sh <ensure|remove> <base url> <audience> <name>}"
base="${2:?base url}"
audience="${3:?audience}"
name="${4:?name}"
: "${POCKETID_API_TOKEN:?POCKETID_API_TOKEN is not set}"
base="${base%/}"

call() { # method path [json body]
  local method="$1" path="$2"
  shift 2
  curl -fsS -X "$method" -H "X-API-KEY: $POCKETID_API_TOKEN" -H 'Content-Type: application/json' \
    ${1:+--data "$1"} "$base/api$path"
}

find_id() {
  call GET "/apis?search=$(jq -rn --arg a "$audience" '$a|@uri')&pagination%5Blimit%5D=100" |
    jq -r --arg a "$audience" '[.data[] | select(.resource == $a)][0].id // empty'
}

id="$(find_id)"
case "$verb" in
ensure)
  if [ -z "$id" ]; then
    id="$(call POST /apis "$(jq -cn --arg n "$name" --arg r "$audience" '{name:$n,resource:$r}')" | jq -r .id)"
    [ -n "$id" ] && [ "$id" != null ] || { echo "pocketid-api.sh: create returned no id for $audience" >&2; exit 1; }
    echo "pocketid-api.sh: registered $audience as $id"
  else
    echo "pocketid-api.sh: adopted existing $audience ($id)"
  fi
  call PUT "/apis/$id/cimd-access" '{"enabled":true,"permissionIds":[]}' >/dev/null
  ;;
remove)
  if [ -n "$id" ]; then
    call DELETE "/apis/$id" >/dev/null
    echo "pocketid-api.sh: removed $audience ($id)"
  fi
  ;;
*) echo "pocketid-api.sh: unknown verb $verb" >&2; exit 1 ;;
esac
