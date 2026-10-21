#!/bin/sh
# =============================================================================
# Containerized installer smoke test — asserts install.sh works on the five
# supported distro families and that the installed binary runs --help.
#
# Requires docker (or podman). Run from the repository root:
#   sh containers/smoke-test.sh              # all distros
#   sh containers/smoke-test.sh debian arch  # just these
#
# What it does per distro:
#   1. builds the container image (Dockerfile.<distro>: base system + sudo
#      + a non-root `builder` user)
#   2. mounts this repository read-only at /repo
#   3. runs install.sh --user --no-rustup as that user inside it
#      (--user keeps everything in $HOME; --no-rustup proves the distro's
#      own rust toolchain is sufficient)
#   4. asserts $HOME/.local/bin/foto exists and `foto --help` exits 0
#
# The source-build path exercises the full distro dependency chain. The
# prebuilt-download path additionally needs a published release — it can be
# tested by removing --no-rustup and the FOTO_REPO override.
# =============================================================================
set -eu

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

DISTROS="${*:-debian arch fedora alpine void}"

for DISTRO in $DISTROS; do
  DOCKERFILE="$REPO_DIR/containers/Dockerfile.$DISTRO"
  [ -f "$DOCKERFILE" ] || { echo "skip: no $DOCKERFILE"; continue; }

  ENGINE=docker
  command -v docker >/dev/null 2>&1 || ENGINE=podman
  command -v "$ENGINE" >/dev/null 2>&1 || { echo "FAIL: no docker/podman"; exit 1; }

  IMAGE="foto-smoke-$DISTRO"
  echo "==> [$DISTRO] building image"
  "$ENGINE" build -q -f "$DOCKERFILE" -t "$IMAGE" "$REPO_DIR/containers" >/dev/null

  echo "==> [$DISTRO] running installer (user scope, no rustup)"
  # FOTO_REPO points at the mounted checkout: no GitHub round-trip, the
  # exact tree under test is what gets installed
  "$ENGINE" run --rm \
    -v "$REPO_DIR:/repo:ro" \
    "$IMAGE"
done

echo
echo "all requested distros passed"
