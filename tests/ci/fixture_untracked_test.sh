#!/usr/bin/env bash
# tier: T0
# covers: deploy/ci.sh tests/ui/fixtures/*
# tests/ci/fixture_untracked_test.sh — no sqlite database is tracked under tests/.
#
# The UI fixture was a committed binary that every catalog/schema bead
# regenerated, so any two of them conflicted on it and git could not merge
# them. It is built from text by tests/ui/fixtures/build.sh instead; this
# keeps a binary from creeping back in.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# CI runs from a copy that may not be a git checkout (git then exits 128); a
# scan of the tree is the same check there, since nothing builds into tests/
# before this step.
if tracked="$(git ls-files 'tests/*.sqlite' 'tests/**/*.sqlite' 'tests/*.db' 'tests/**/*.db' 2>/dev/null)"; then
    :
else
    tracked="$(find tests \( -name '*.sqlite' -o -name '*.db' \) -type f)"
fi
if [ -n "$tracked" ]; then
    echo "FAIL: binary databases tracked under tests/ (build them from text):" >&2
    echo "$tracked" >&2
    exit 1
fi
echo "ok: no database is tracked under tests/"
