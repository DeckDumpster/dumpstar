# Dumpstar container image — multi-stage build.
#
# Build:  podman build -t dumpstar:latest -f Containerfile .
# The image runs `dumpstar serve`; populate the catalog with deploy/seed.sh.

# --- Stage 1: Rust build ----------------------------------------------------
# The Debian release is pinned, not inherited. `rust:1.94-slim` is a MOVING
# tag: upstream retagged it from bookworm to trixie, the builder started
# linking against glibc 2.39, and every image built after that shipped a
# binary the bookworm runtime stage below cannot exec (pd-pejn). The invariant
# is builder glibc <= runtime glibc, and the only thing that holds it is these
# two FROM lines naming the same release. Change one, change the other — and
# the target cache id below with them.
# The default is a non-executable placeholder, which the build reads as "no cache".
FROM docker.io/library/debian:bookworm-slim AS sccache-default
RUN mkdir /out && : > /out/sccache
LABEL dumpstar.build="1"
FROM scratch AS sccache-bin
COPY --from=sccache-default /out/ /
LABEL dumpstar.build="1"

FROM docker.io/library/rust:1.94-slim-bookworm AS builder

WORKDIR /app

# Workspace manifests, sources, and the embedded seed data
# (data/*.json is pulled in at compile time via include_str!).
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY data/ data/

# BuildKit cache mounts: the cargo registry + the workspace's target/ both
# persist across rebuilds, so a small Rust change incrementally compiles in
# seconds instead of triggering a 3-4 minute release rebuild from scratch.
# `cp` the binary out at the end because the cache mount disappears after
# the RUN, taking target/release/dumpstar with it.
#
# The target cache is scoped by `id=` on TWO axes, and both are load-bearing for
# the same reason: cargo fingerprints record crate versions, rustc version and
# source mtimes — they do NOT record which base image or which CHECKOUT produced
# the objects.
#
#   * the builder's DEBIAN RELEASE. When the base drifted (pd-pejn), a shared
#     cache handed the next build the objects compiled on trixie, cargo relinked
#     nothing, and `cp` copied the unrunnable binary straight through — so
#     re-pinning the FROM line above looked applied and changed nothing, on every
#     box that had already built once. A base-image change must change this id.
#
#   * the CHECKOUT (pd-sjn7). One box runs the rig root, a CI runner and any
#     number of polecat worktrees, every one of them building /app from its own
#     sources. With a constant id they share one target directory, and a worktree
#     whose rlib is merely NEWER wins: the next build reuses it, relinks nothing,
#     and fails on whatever that rlib does not export — while COPY demonstrably
#     delivered the right sources, which is a maddening thing to debug. Worse, a
#     stale rlib that still exports everything referenced links CLEANLY and ships
#     old behaviour, so a gate can go green against code nobody wrote.
#
# CARGO_TARGET_CACHE_SCOPE is that second axis. `deploy/image-lib.sh` passes a
# hash of the checkout path — the same per-checkout suffix every container gate
# already derives for its network, volume and image names — so each tree keeps
# its own warm cache and can poison nobody else's. Every gate in ONE ci.sh run
# shares a checkout and therefore still shares the cache, which is the whole
# benefit the id was introduced to have.
#
# The default is deliberately not a hash: a bare `podman build` cannot compute
# one, so it says so instead of silently claiming a scope it does not have.
#
# THOSE TWO AXES ARE NOT ENOUGH ON THEIR OWN, and the third is the SOURCE TREE
# (pd-wjmd). A CI runner is ONE checkout path holding EVERY tree over time — a
# different PR merge ref on every run — so the id is constant while the sources
# under it are not. Worse, a COPY that HITS the layer cache restores that
# layer's historical mtimes rather than the build context's, so the sources can
# arrive OLDER than rlibs a later run compiled from a different tree. Cargo's
# freshness check is an mtime comparison; it then calls them fresh, compiles
# nothing, and `cp` copies the OTHER tree's binaries straight into the image.
#
# Measured, on run 33683221574: a build whose crates/ had no `sets.ptcgio_covered`
# reported `Finished release profile in 1.92s` and shipped a dumpstar-lake-derive
# that writes that column. The container gate below it caught it only by
# accident, as a catalog disagreeing with the one the host built.
#
# So the cache carries a STAMP of the sources it was compiled from. A build
# handed a cache written from anything else touches the workspace sources first,
# which is the one thing that makes cargo's mtime comparison tell the truth
# again. When the stamp matches, nothing is touched and the warm cache is worth
# exactly what it was worth before. The stamp is removed before the build and
# written after it, so a build that FAILED leaves no claim about what the cache
# holds. `DUMPSTAR_SOURCES` is the one spelling of "what this stage compiles from"
# — it is both what is hashed and what is touched, and tests/deploy/run.sh
# requires every COPY into this stage to appear in it.
#
# FOUR binaries, built in one invocation so they share the compile:
#
#   dumpstar             the app. `serve` is the entrypoint; `data refresh` is
#                      what the nightly LANDING unit runs. Reads no raw/.
#   dumpstar-lake-derive the offline catalog derive (pd-1uem). The only thing in
#                      the image that reads raw/, and the only thing that can:
#                      it is a bin-only crate, so `dumpstar` cannot link it.
#   dumpstar-ship        the shipper (pd-dxn3): the ownership outbox into the
#                      tenant zone. Offline, like the derive — nothing serving
#                      a request runs it, and the entrypoint above cannot
#                      become it by accident.
#   dumpstar-erase       the deletion path (pd-qbrf): tombstone the key, drop the
#                      tenant's partition, and prove it unreadable. Offline for
#                      the same two reasons the shipper is — it needs the tenant
#                      credentials and the master key, and neither belongs on a
#                      process that answers requests.
#
# They ship in ONE image deliberately, even though the whole point of the crate
# split is that the halves can eventually run on different machines. The image
# is not the boundary — the process is, and so are the credentials each process
# is handed. Shipping one image keeps every offline job on exactly the build
# that wrote the schema it reads, which is what stops an offline job and an
# online server disagreeing about the shape of a database; splitting the images
# is a deployment change to make when the machines actually split, not before.
ARG CARGO_TARGET_CACHE_SCOPE=unscoped
# The shared compile cache (sp-2i16qf). `sccache-bin` is a named build context
# deploy/image-lib.sh supplies (the host's static sccache); without it the stage
# below is an empty directory and the release build simply compiles cold.
# SCCACHE_WEBDAV_ENDPOINT is the store, empty meaning "no cache".
ARG SCCACHE_WEBDAV_ENDPOINT=
ENV DUMPSTAR_SOURCES="Cargo.toml Cargo.lock rust-toolchain.toml crates data"
RUN --mount=type=bind,from=sccache-bin,source=/,target=/opt/sccache-ctx \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target,sharing=locked,id=dumpstar-target-bookworm-${CARGO_TARGET_CACHE_SCOPE} \
    stamp="$(find ${DUMPSTAR_SOURCES} -type f -exec sha256sum {} + \
             | LC_ALL=C sort | sha256sum | cut -d' ' -f1)" \
 && if [ "$(cat target/.dumpstar-source-stamp 2>/dev/null)" != "$stamp" ]; then \
        echo "==> the cargo target cache was written from other sources — restamping mtimes"; \
        find ${DUMPSTAR_SOURCES} -type f -exec touch {} +; \
    fi \
 && rm -f target/.dumpstar-source-stamp \
 && cache=off; why=; \
    if [ -z "$SCCACHE_WEBDAV_ENDPOINT" ]; then why="no SCCACHE_WEBDAV_ENDPOINT build arg (the host's cache was off: see the 'rust cache:' line earlier in the run)"; \
    elif [ ! -s /opt/sccache-ctx/sccache ]; then why="the sccache-bin build context is empty"; \
    else \
        cp /opt/sccache-ctx/sccache /tmp/sccache && chmod 755 /tmp/sccache; \
        if /tmp/sccache --version >/dev/null 2>&1; then cache=on; \
        else why="the sccache binary does not execute here: $(/tmp/sccache --version 2>&1 | head -1)"; fi; \
    fi \
 && if [ "$cache" = on ]; then \
        echo "==> rust cache: sccache -> $SCCACHE_WEBDAV_ENDPOINT"; \
        export RUSTC_WRAPPER=/tmp/sccache SCCACHE_WEBDAV_KEY_PREFIX=/ \
               SCCACHE_IGNORE_SERVER_IO_ERROR=1 CARGO_INCREMENTAL=0; \
    else \
        echo "==> rust cache: not in use for the image build — compiling cold: $why"; \
    fi \
 && cargo build --release --locked --bin dumpstar --bin dumpstar-lake-derive \
        --bin dumpstar-ship --bin dumpstar-erase \
 && if [ "$cache" = on ]; then echo "==> rust cache summary (image build):"; \
        /tmp/sccache --show-stats | grep -E '^Cache (hits|misses) |^Cache hits rate'; fi \
 && printf '%s\n' "$stamp" > target/.dumpstar-source-stamp \
 && mkdir -p /out \
 && cp target/release/dumpstar /out/dumpstar \
 && cp target/release/dumpstar-lake-derive /out/dumpstar-lake-derive \
 && cp target/release/dumpstar-ship /out/dumpstar-ship \
 && cp target/release/dumpstar-erase /out/dumpstar-erase

# Every image this build produces carries this label, stage images included.
# It is what confines the orphan collection in deploy/image-lib.sh to layers
# THIS repo orphaned: a dangling image without it belongs to something else
# sharing the store, and prod's store is shared. See dumpstar_image_orphans.
LABEL dumpstar.build="1"

# --- Stage 2: SvelteKit build -----------------------------------------------
# adapter-static emits the SPA to frontend/build. The committed ts-rs types in
# frontend/src/lib/types/ mean this stage needs nothing from the Rust build.
# Pinned to a release for the same reason as the builder, though this stage is
# far less exposed — it emits static files, not a linked binary.
FROM docker.io/library/node:22-bookworm-slim AS frontend

WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./
# The remedial aspect icons; the build copies them into static/aspects/.
COPY data/remedial/aspects/ ../data/remedial/aspects/
# The remedial inline game icons; the build copies them into static/icons/.
COPY data/remedial/inline-icons/ ../data/remedial/inline-icons/
RUN npm run build

LABEL dumpstar.build="1"

# --- Stage 3: runtime -------------------------------------------------------
FROM docker.io/library/debian:bookworm-slim

# ca-certificates for HTTPS during `dumpstar setup` / `dumpstar data refresh`. No
# OpenSSL needed — reqwest is built with rustls. sqlite3 is the operator's
# CLI for looking at /data in place (deploy/README.md, "Inspecting prod").
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates sqlite3 \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out/dumpstar /usr/local/bin/dumpstar
COPY --from=builder /out/dumpstar-lake-derive /usr/local/bin/dumpstar-lake-derive
COPY --from=builder /out/dumpstar-ship /usr/local/bin/dumpstar-ship
COPY --from=builder /out/dumpstar-erase /usr/local/bin/dumpstar-erase
COPY --from=frontend /app/frontend/build /srv/dumpstar/static

# Catalog + per-user databases live on a mounted volume.
ENV DUMPSTAR_HOME=/data
# Directory holding the built SvelteKit SPA, served by `dumpstar serve`.
ENV DUMPSTAR_STATIC_DIR=/srv/dumpstar/static
EXPOSE 8080

LABEL dumpstar.build="1"
# Links the published package (ghcr.io/deckdumpster/dumpstar-image) to this repo,
# so it inherits the repo's visibility and the prod host can pull it (sp-jooqc0).
LABEL org.opencontainers.image.source="https://github.com/DeckDumpster/dumpstar"

ENTRYPOINT ["dumpstar", "serve", "--host", "0.0.0.0", "--port", "8080"]
