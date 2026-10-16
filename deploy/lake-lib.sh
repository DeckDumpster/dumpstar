#!/usr/bin/env bash
#
# Is a lakehouse installed for an instance? Sourced, never executed.
#
# The Python lake (lake/, its job image, prices.sh, value-snapshots.sh,
# nessie-backup.sh, setup-lake.sh) was deleted on 2026-10-16 (sp-l74uk0): it never
# ran in production — no lake job image, no armed lake timers — and the lake is
# being rebuilt in Rust on dumpster-kit. What remains is the one question
# deploy/data-jobs-lib.sh still asks before it runs a lake job.
#
# The Nessie Quadlet unit is the marker, deliberately, not an image: a box with
# no lakehouse is the ordinary case, and nothing on the serving path touches one.
dumpstar_lake_installed() {
    [ -f "${HOME}/.config/containers/systemd/dumpstar-nessie-${1}.container" ]
}
