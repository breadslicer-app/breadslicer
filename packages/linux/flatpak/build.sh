#!/bin/sh

# Build the SoulCrafted Slicer Flatpak.
#
# flatpak-builder never caches a `dir` source, so an unchanged rebuild would
# repeat the whole compile. Instead this packs the working tree (without
# target/) into a deterministic tarball and builds from it as an `archive`
# source. flatpak-builder keys its build cache on the checksum a source
# declares, never on the file's own content, so the tarball is accompanied by a
# generated source list carrying its sha256: an unchanged re-run is skipped,
# and a changed working tree recompiles. Downloaded crate and Skia archives are
# cached under .cache/ too, so only the first build of a given source pays the
# cost.
#
# The build goes into a local OSTree repo under .cache/repo, which is then
# exported to a single-file .flatpak bundle under .cache/. This does not install
# anything; install the bundle with `flatpak install --user <path>`.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
MANIFEST="$SCRIPT_DIR/it.soulcrafted.slicer.yml"
CACHE_NAME=.cache
CACHE="$SCRIPT_DIR/$CACHE_NAME"
SRC_DIR="$CACHE/src"
TARBALL_RELPATH="$CACHE_NAME/source.tar"
TARBALL="$SCRIPT_DIR/$TARBALL_RELPATH"
SOURCE_LIST="$CACHE/source.source.json"
APP_ID=it.soulcrafted.slicer
REPO="$CACHE/repo"
BUNDLE="$CACHE/$APP_ID.flatpak"

echo "Refreshing a clean source copy (without target/) ..."
mkdir -p "$SRC_DIR"
# Leave out target/, every top-level dot directory (.git and the like), and
# the build caches; none of them belong in the build.
rsync -a --checksum --delete \
    --exclude='/target' \
    --exclude='/.*' \
    --exclude='.cache' \
    --exclude='.flatpak-builder' \
    "$REPO_ROOT"/ "$SRC_DIR"/

echo "Packing it deterministically (so an unchanged source reuses the cache) ..."
tar --sort=name --mtime='2020-01-01 00:00:00' --owner=0 --group=0 --numeric-owner \
    -cf "$TARBALL" -C "$SRC_DIR" .

# The checksum is what invalidates the cached build, so it is written next to
# the tarball on every run. A module resolves the path of every source against
# the manifest's directory, including the sources an included list brings in,
# so the tarball is named relative to the manifest rather than to this list.
echo "Recording its checksum ..."
cat > "$SOURCE_LIST" <<EOF
[
  {
    "type": "archive",
    "path": "$TARBALL_RELPATH",
    "sha256": "$(sha256sum "$TARBALL" | cut -d ' ' -f 1)",
    "strip-components": 0
  }
]
EOF

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
