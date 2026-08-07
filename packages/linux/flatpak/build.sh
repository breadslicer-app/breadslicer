#!/bin/sh

# Build the SoulCrafted Slicer Flatpak packages: the GUI and the CLI.
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
# The builds go into a local OSTree repo under .cache/repo, which is then
# exported to single-file .flatpak bundles under .cache/. This does not install
# anything; install a bundle with `flatpak install --user <path>`.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
CACHE_NAME=.cache
CACHE="$SCRIPT_DIR/$CACHE_NAME"
SRC_DIR="$CACHE/src"
TARBALL_RELPATH="$CACHE_NAME/source.tar"
TARBALL="$SCRIPT_DIR/$TARBALL_RELPATH"
SOURCE_LIST="$CACHE/source.source.json"
METAINFO_NAME=it.soulcrafted.slicer.metainfo.xml
REPO="$CACHE/repo"
# Both packages are built from the same tarball, each from the manifest named
# after it. Flatpak exports a single command per application id, named after
# that id, so the command-line tool needs an id of its own to reach the host
# PATH; it cannot share the GUI's.
APP_IDS='it.soulcrafted.slicer it.soulcrafted.slicer.cli'

# The vendored declarations are what the sandbox builds from, so a stale one
# fails deep inside the compile with no network to recover from. Both are
# derived from Cargo.lock, so this decides it up front, before any work.
echo "Checking the vendored sources ..."
sh "$SCRIPT_DIR/check-vendored-sources.sh"

# The metainfo names the version being built and the date of the build, so it
# is rendered from the template rather than kept up to date by hand. It is
# declared as a source of its own, and installed by a module of its own, so
# that a new date invalidates only that module and leaves the cached compile
# alone.
echo "Rendering the metainfo ..."
mkdir -p "$CACHE"
sh "$REPO_ROOT/scripts/render-metainfo.sh" \
    "$REPO_ROOT/packages/shared/$METAINFO_NAME" \
    "$REPO_ROOT/apps/slicer-gui/Cargo.toml" \
    "$CACHE/$METAINFO_NAME"

cat > "$CACHE/metainfo.source.json" <<EOF
[
  {
    "type": "file",
    "path": "$CACHE_NAME/$METAINFO_NAME",
    "sha256": "$(sha256sum "$CACHE/$METAINFO_NAME" | cut -d ' ' -f 1)"
  }
]
EOF

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

for APP_ID in $APP_IDS; do
    echo "Building $APP_ID ..."
    flatpak-builder --user --force-clean \
        --state-dir="$CACHE/flatpak-builder" \
        --repo="$REPO" \
        "$CACHE/build/$APP_ID" \
        "$SCRIPT_DIR/$APP_ID.yml"
done

echo "Exporting the bundles ..."
for APP_ID in $APP_IDS; do
    flatpak build-bundle "$REPO" "$CACHE/$APP_ID.flatpak" "$APP_ID"
done

echo
for APP_ID in $APP_IDS; do
    echo "Built: $CACHE/$APP_ID.flatpak"
done
echo "Install each with: flatpak install --user <path>"
