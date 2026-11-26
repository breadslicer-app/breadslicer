#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
#
# Builds, tests and packages Breadslicer for Linux in Docker containers, from
# Linux or from Windows (Git Bash with Docker Desktop).
#
#   packages/linux/build.sh [test|package|smoke|all] [out-dir]
#
#   test     workspace build and every test, on Arch, under Xvfb with
#            software rendering (Mesa lavapipe).
#   package  release build on Ubuntu 22.04 (glibc 2.35), then the .tar.gz,
#            the AppImage and the PKGBUILD.
#   smoke    on Arch: builds and installs the Arch package from the .tar.gz,
#            starts the app under Xvfb with a model and takes a screenshot.
#   all      the three, in that order (the default).
#
# The packages and the logs land in out-dir (default target/linux-packages).
# The source is copied into a Docker volume (the working tree as it is,
# uncommitted changes included, LF line endings); the target folders and the
# cargo registry live in volumes too, never in a folder of the host.
set -euo pipefail

step="${1:-all}"
root="$(git rev-parse --show-toplevel)"
out="${2:-$root/target/linux-packages}"
mkdir -p "$out"
here="$root/packages/linux"
export MSYS_NO_PATHCONV=1
host_path() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
out_mount="$(host_path "$(cd "$out" && pwd)")"

ARCH_IMAGE=breadslicer-linux-arch
PORTABLE_IMAGE=breadslicer-linux-portable

image() {
    docker build -q -f "$(host_path "$here/docker/$2")" -t "$1" "$(host_path "$here/docker")" >/dev/null
}

# The working tree (tracked and untracked files, not the ignored ones) into the
# source volume; only files whose content changed are rewritten, so cargo
# rebuilds only what changed.
sync_source() {
    local index tree
    index="$(mktemp -u)"
    GIT_INDEX_FILE="$index" git -C "$root" add -A
    tree="$(GIT_INDEX_FILE="$index" git -C "$root" write-tree)"
    rm -f "$index"
    git -C "$root" -c core.autocrlf=false archive --format=tar "$tree" \
        | docker run --rm -i -v breadslicer-linux-src:/src "$ARCH_IMAGE" sh -c \
            'rm -rf /tmp/s && mkdir /tmp/s && tar -x -C /tmp/s && rsync -rc --delete /tmp/s/ /src/'
}

# Runs `$3...` in image `$1` with target volume `$2`.
run_in() {
    local img="$1" target="$2"
    shift 2
    docker run --rm \
        -v breadslicer-linux-src:/src \
        -v "$target":/target \
        -v breadslicer-linux-cargo:/cargo-home \
        -v "$out_mount":/out \
        -e CARGO_HOME=/cargo-home -e CARGO_TARGET_DIR=/target -e CARGO_INCREMENTAL=0 \
        "$img" "$@"
}

# The Arch image also copies the source in, so it is always built.
image "$ARCH_IMAGE" arch.Dockerfile
case "$step" in
    package | all) image "$PORTABLE_IMAGE" portable.Dockerfile ;;
esac
sync_source

case "$step" in
    test) run_in "$ARCH_IMAGE" breadslicer-linux-target bash /src/packages/linux/ci/test.sh ;;
    package) run_in "$PORTABLE_IMAGE" breadslicer-linux-target-portable bash /src/packages/linux/ci/package.sh ;;
    smoke) run_in "$ARCH_IMAGE" breadslicer-linux-target bash /src/packages/linux/ci/smoke.sh ;;
    all)
        run_in "$ARCH_IMAGE" breadslicer-linux-target bash /src/packages/linux/ci/test.sh
        run_in "$PORTABLE_IMAGE" breadslicer-linux-target-portable bash /src/packages/linux/ci/package.sh
        run_in "$ARCH_IMAGE" breadslicer-linux-target bash /src/packages/linux/ci/smoke.sh
        ;;
    *) echo "unknown step: $step (test, package, smoke or all)" >&2; exit 2 ;;
esac
echo "out: $out"
