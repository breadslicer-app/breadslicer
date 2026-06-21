#!/bin/sh

# Obtain the prebuilt Skia static library that skia-bindings links, so an
# offline build (a Flatpak build has no network access) can link Skia
# without skia-bindings reaching out to the network at build time.
#
# skia-bindings downloads a prebuilt archive from the rust-skia
# skia-binaries releases, keyed by a hash of its build configuration. This
# script resolves the exact archive for the configuration the workspace
# pins, downloads it, records its checksum as a Flatpak `file` source, and
# leaves the build to consume it via SKIA_BINARIES_URL.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
CACHE_DIR="$SCRIPT_DIR/.cache"
SOURCE_FILE="$SCRIPT_DIR/skia-binaries.source.json"
DEST_FILENAME="skia-binaries.tar.gz"

# A prebuilt Skia archive is not published for every target. When it is
# missing, building Skia from source is the fallback; that path is not
# implemented yet (see the rust-skia FORCE_SKIA_BUILD documentation).
build_from_source() {
    echo "error: no prebuilt Skia archive is available for this build" >&2
    echo "       configuration; building Skia from source is not supported" >&2
    echo "       yet." >&2
    exit 1
}

# Resolve the skia-binaries archive URL for the configuration the
# workspace pins by reading the URL skia-bindings recorded in its build
# output. Requires that the workspace has been built at least once.
resolve_archive_url() {
    urls=$(grep -hE '^[[:space:]]*FROM:' \
        "$REPO_ROOT"/target/*/build/skia-bindings-*/output 2>/dev/null \
        | sed -E 's/^[[:space:]]*FROM:[[:space:]]*//' \
        | sort -u || true)

    if [ -z "$urls" ]; then
        echo "error: no skia-bindings build output found under target/." >&2
        echo "       Build the workspace once (cargo build) so skia-bindings" >&2
        echo "       records the archive URL, then re-run this script." >&2
        exit 1
    fi

    if [ "$(printf '%s\n' "$urls" | grep -c .)" -ne 1 ]; then
        echo "error: skia-bindings recorded more than one archive URL; the" >&2
        echo "       Skia build configuration is ambiguous:" >&2
        printf '%s\n' "$urls" | sed 's/^/         /' >&2
        exit 1
    fi

    printf '%s\n' "$urls"
}

obtain_prebuilt() {
    url=$1
    archive="$CACHE_DIR/$(basename "$url")"

    mkdir -p "$CACHE_DIR"
    if ! curl --proto '=https' --tlsv1.2 -fL --retry 2 -o "$archive.tmp" "$url"; then
        rm -f "$archive.tmp"
        build_from_source
    fi
    mv -f "$archive.tmp" "$archive"

    sha256=$(sha256sum "$archive" | cut -d ' ' -f 1)

    cat > "$SOURCE_FILE" <<EOF
[
  {
    "type": "file",
    "url": "$url",
    "sha256": "$sha256",
    "dest-filename": "$DEST_FILENAME"
  }
]
EOF

    echo "Wrote $SOURCE_FILE"
    echo "Cached archive at $archive"
    echo
    echo "The build links this archive offline by setting, in the directory"
    echo "where the archive was placed:"
    echo "  SKIA_BINARIES_URL=file://\$(pwd)/$DEST_FILENAME"
}

obtain_prebuilt "$(resolve_archive_url)"
