#!/usr/bin/env bash
# Guard (sp-7y6mzt): test fixtures are Rust. No committed Python, no python3
# in the test harnesses. The allowlist only shrinks.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
fail=0
py="$(git ls-files '*.py')"
if [[ -n "$py" ]]; then echo "FAIL: committed Python:" >&2; echo "$py" >&2; fail=1; fi
hits="$(git ls-files 'tests/*.sh' | grep -v '^tests/ci/no_python_test.sh$' | xargs grep -ln '\bpython3\?\b' 2>/dev/null || true)"
allow='^tests/ci/select_test\.sh$'
bad="$(grep -Ev "$allow" <<<"$hits" || true)"
if [[ -n "$bad" ]]; then echo "FAIL: python in tests:" >&2; echo "$bad" >&2; fail=1; fi
((fail)) || echo "ok: no Python in the test tree"
exit "$fail"
