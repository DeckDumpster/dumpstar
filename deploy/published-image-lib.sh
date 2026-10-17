#!/usr/bin/env bash
#
# The shipped image is built once, by CI, and prod pulls that build (sp-jooqc0).
#
# The deploy used to run `podman build` on the production host: seven rustc
# processes at once when two repos deployed together, 6-14 minute deploys, and a
# binary prod compiled for itself rather than the one the gate tested
# (law-deploy-the-tested-artifacts). deckdumpster's deploy finishes in under a
# minute because its image is pulled, not built; this is the same shape.
#
# The key is the TREE of the commit, not its sha. CI tests the PR's merge ref and
# the queue lands a merge commit with a different sha but, when main has not moved
# under it, the same tree - and the tree is exactly what the image is built from.
#
#   ghcr.io/deckdumpster/dumpstar-image:tree-<40 hex>
#
# CI pushes it (deploy/ci.sh, DUMPSTAR_PUBLISH_IMAGE=1); the deploy pulls it.
#
#   DUMPSTAR_DEPLOY_IMAGE=auto   (default) pull; if the tree was never published
#                                (a run that built no image, main moved under the
#                                merge), say so loudly and build here.
#   DUMPSTAR_DEPLOY_IMAGE=pull   pull or fail the deploy. No compile on this host.
#   DUMPSTAR_DEPLOY_IMAGE=build  always build here (the old behaviour; forced mode).

DUMPSTAR_IMAGE_REPO="${DUMPSTAR_IMAGE_REPO:-ghcr.io/deckdumpster/dumpstar-image}"

# dumpstar_published_ref <repo_dir>  - prints the reference, fails if no tree.
dumpstar_published_ref() {
	local tree
	tree="$(git -C "$1" rev-parse 'HEAD^{tree}' 2>/dev/null)" || return 1
	printf '%s:tree-%s\n' "$DUMPSTAR_IMAGE_REPO" "$tree"
}

# dumpstar_registry_login  - when GHCR_TOKEN is set; the package is public, so
# a deploy without one still pulls.
dumpstar_registry_login() {
	[ -n "${GHCR_TOKEN:-}" ] || return 0
	printf '%s' "$GHCR_TOKEN" |
		podman login ghcr.io -u "${GHCR_USER:-dumpstar}" --password-stdin >/dev/null
}

# dumpstar_image_publish <local image> <repo_dir>
dumpstar_image_publish() {
	local image="$1" ref
	ref="$(dumpstar_published_ref "$2")" || {
		echo "ERROR: cannot name the tree of $2; the image is not published." >&2
		return 1
	}
	dumpstar_registry_login || return 1
	podman tag "$image" "$ref" || return 1
	podman push "$ref" || return 1
	echo "==> published $ref ($(podman image inspect --format '{{.Id}}' "$ref" | cut -c1-12))"
}

# dumpstar_image_fetch <repo_dir>
#
# Prints (stdout) the pulled reference when the deploy should ship it; prints
# nothing and returns 0 when it should build; returns 1 to fail the deploy.
dumpstar_image_fetch() {
	local mode="${DUMPSTAR_DEPLOY_IMAGE:-auto}" ref
	case "$mode" in
		build) return 0 ;;
		auto | pull) ;;
		*) echo "ERROR: DUMPSTAR_DEPLOY_IMAGE=${mode} is not auto, pull or build." >&2; return 1 ;;
	esac
	if ! ref="$(dumpstar_published_ref "$1")"; then
		echo "==> no git tree to key the image by" >&2
		[ "$mode" = pull ] && return 1
		return 0
	fi
	dumpstar_registry_login >&2 || true
	if podman image exists "$ref" >&2 || podman pull -q "$ref" >&2; then
		echo "==> shipping CI's image ${ref} ($(podman image inspect --format '{{.Id}}' "$ref" | cut -c1-12))" >&2
		printf '%s\n' "$ref"
		return 0
	fi
	echo "==> ${ref} is not published (CI built no image for this tree)" >&2
	if [ "$mode" = pull ]; then
		echo "ERROR: DUMPSTAR_DEPLOY_IMAGE=pull and there is nothing to pull; not compiling on this host." >&2
		return 1
	fi
	echo "WARNING: falling back to building on this host." >&2
	return 0
}
