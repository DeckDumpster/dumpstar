#!/usr/bin/env bash
# tier: T0
# covers: tests/ci/no_python.sh
# tests/ci/no_python.sh [--self-test] — dumpstar is a Rust repository and public: no tracked *.py
# (law-one-language-runtime-per-repo; sp-io9c7k). Names every offending file and fails.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"

check() {
    local offenders
    offenders="$(git -C "$1" ls-files -- '*.py')"
    if [ -n "$offenders" ]; then
        echo "FAIL: tracked Python files (this repository has no Python; write it in Rust or shell):" >&2
        printf '  %s\n' $offenders >&2
        return 1
    fi
    echo "  PASS  no tracked *.py"
}

if [ "${1:-}" = --self-test ]; then
    t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
    git -C "$t" init -q
    echo 'print(1)' >"$t/x.py"; git -C "$t" add x.py
    if check "$t" 2>/dev/null; then echo "  FAIL  a tracked .py passed the guard"; exit 1; fi
    echo "  PASS  a tracked .py is refused"
    git -C "$t" rm -qf x.py
    check "$t"
    exit 0
fi
check "${1:-$REPO}"
