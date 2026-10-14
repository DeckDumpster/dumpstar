#!/usr/bin/env bash
#
# Print the failed tests of the last visual run, and the first line of each
# error, as markdown. The record-baselines workflow appends it to the job
# summary so a failed recording can be acted on without forge access.
#
# Usage: bash tests/visual/failure-summary.sh [results.json]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RESULTS="${1:-$SCRIPT_DIR/results.json}"

echo "## Visual run failures"
echo
if [ ! -f "$RESULTS" ]; then
    echo "No \`results.json\` was written: the run failed before Playwright reported (instance build, server start or install)."
    exit 0
fi

node -e '
const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const failed = [];
const walk = (suite) => {
  for (const spec of suite.specs ?? [])
    for (const t of spec.tests ?? []) {
      const bad = (t.results ?? []).filter((x) => ["failed", "timedOut"].includes(x.status));
      if (!bad.length || t.status === "expected") continue;
      const msg = (bad[0].error?.message ?? "").replace(/\x1b\[[0-9;]*m/g, "");
      const line = msg.split("\n").find((l) => l.trim()) ?? "(no error message)";
      failed.push(`- \`${t.projectName}\` ${spec.title}: ${line.trim()}`);
    }
  for (const s of suite.suites ?? []) walk(s);
};
for (const s of r.suites ?? []) walk(s);
if (!failed.length) console.log("No failed tests in results.json.");
else {
  console.log(`${failed.length} failed test(s):\n`);
  console.log(failed.join("\n"));
}
' "$RESULTS"
