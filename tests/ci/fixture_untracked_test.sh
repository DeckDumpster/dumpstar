#!/usr/bin/env bash
# tests/ci/fixture_untracked_test.sh — no sqlite database is tracked under tests/.
#
# The UI fixture was a committed binary that every catalog/schema bead
# regenerated, so any two of them conflicted on it and git could not merge
# them. It is built from text by tests/ui/fixtures/build.sh instead; this
# keeps a binary from creeping back in.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
tracked="$(git ls-files 'tests/*.sqlite' 'tests/**/*.sqlite' 'tests/*.db' 'tests/**/*.db')"
if [ -n "$tracked" ]; then
    echo "FAIL: binary databases tracked under tests/ (build them from text):" >&2
    echo "$tracked" >&2
    exit 1
fi
echo "ok: no database is tracked under tests/"
