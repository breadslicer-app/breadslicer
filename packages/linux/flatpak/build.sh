#!/bin/sh

# Build the SoulCrafted Slicer Flatpak.
#
# flatpak-builder never caches a `dir` source, so an unchanged rebuild would
# repeat the whole compile. Instead this packs the working tree (without
# target/) into a deterministic tarball and builds from it as an `archive`
# source, which flatpak-builder caches by content: an unchanged re-run is
# skipped. Downloaded crate and Skia archives are cached under .cache/ too, so
# only the first build of a given source pays the cost.
#
# The build goes into a local OSTree repo under .cache/repo, which is then
# exported to a single-file .flatpak bundle under .cache/. This does not install
# anything; install the bundle with `flatpak install --user <path>`.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
MANIFEST="$SCRIPT_DIR/it.soulcrafted.Slicer.yml"
CACHE="$SCRIPT_DIR/.cache"
SRC_DIR="$CACHE/src"
TARBALL="$CACHE/source.tar"
APP_ID=it.soulcrafted.Slicer
REPO="$CACHE/repo"
BUNDLE="$CACHE/$APP_ID.flatpak"

echo "Refreshing a clean source copy (without target/) ..."
mkdir -p "$SRC_DIR"
# Leave out target/, every top-level dot directory (.git and the like), and
# the build caches; none of them belong in the build.
rsync -a --delete \
    --exclude='/target' \
    --exclude='/.*' \
    --exclude='.cache' \
    --exclude='.flatpak-builder' \
    "$REPO_ROOT"/ "$SRC_DIR"/

echo "Packing it deterministically (so an unchanged source reuses the cache) ..."
tar --sort=name --mtime='2020-01-01 00:00:00' --owner=0 --group=0 --numeric-owner \
    -cf "$TARBALL" -C "$SRC_DIR" .

echo "Building ..."
flatpak-builder --user --force-clean \
    --state-dir="$CACHE/flatpak-builder" \
    --repo="$REPO" \
    "$CACHE/build" \
    "$MANIFEST"

echo "Exporting the bundle ..."
flatpak build-bundle "$REPO" "$BUNDLE" "$APP_ID"

echo
echo "Built: $BUNDLE"
echo "Install it with: flatpak install --user \"$BUNDLE\""
