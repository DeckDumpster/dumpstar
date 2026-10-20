#!/usr/bin/env bash
# tier: T1
# covers: deploy/dashboards/*.json
# Guard (sp-typbus): the Grafana dashboard the product metrics stack provisions is valid JSON,
# has the panels the bead names, queries only metrics the server sends, draws the 200 ms line
# on search page latency, and groups by no tenant or email attribute.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
f=deploy/dashboards/dumpstar.json
fail=0
bad() { echo "FAIL: $*" >&2; fail=1; }
jq -e . "$f" >/dev/null || { echo "FAIL: $f is not valid JSON" >&2; exit 1; }
[[ "$(jq -r .uid "$f")" == dumpstar ]] || bad "uid must be dumpstar"
jq -e '.panels | length >= 5 and all(.[]; (.title|length>0) and (.targets|length>0) and all(.targets[]; (.expr|length>0)))' "$f" >/dev/null || bad "panels must have a title and non-empty queries"
jq -e '[.panels[].id] | (length == (unique|length))' "$f" >/dev/null || bad "duplicate panel ids"
for want in 'Request rate' 'Request latency' 'Search page latency' 'Search totals' 'Error rate'; do
	jq -e --arg w "$want" 'any(.panels[]; .title | startswith($w))' "$f" >/dev/null || bad "no panel titled '$want…'"
done
jq -e '.panels[] | select(.title | startswith("Search page latency")) | .fieldConfig.defaults | (.thresholds.steps | any(.value == 200)) and (.custom.thresholdsStyle.mode | test("line"))' "$f" >/dev/null || bad "search page latency lacks the 200 ms threshold line"
metrics="$(jq -r '.panels[].targets[].expr' "$f" | grep -oE '(dumpstar|http_server)_[a-z_]+' | sed -E 's/_(bucket|sum|count)$//' | sort -u)"
for m in $metrics; do
	case "$m" in
	http_server_requests_total | http_server_request_duration_seconds | dumpstar_search_page_ms | dumpstar_search_totals_ms | dumpstar_search_rows) ;;
	*) bad "dashboard queries unknown metric $m" ;;
	esac
done
grep -qiE 'tenant|email|user' <<<"$(jq -r '.panels[].targets[] | .expr + " " + .legendFormat' "$f")" && bad "a query or legend names a tenant, email or user" || true
((fail)) || echo "ok: dashboard $f"
exit "$fail"
