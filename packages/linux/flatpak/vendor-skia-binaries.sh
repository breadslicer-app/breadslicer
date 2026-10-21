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

if ! command -v jq >/dev/null 2>&1; then
    echo "error: jq is required to read the build messages cargo reports." >&2
    exit 1
fi

# A prebuilt Skia archive is not published for every target. When it is
# missing, building Skia from source is the fallback; that path is not
# implemented yet (see the rust-skia FORCE_SKIA_BUILD documentation).
build_from_source() {
    echo "error: no prebuilt Skia archive is available for this build" >&2
    echo "       configuration; building Skia from source is not supported" >&2
    echo "       yet." >&2
    exit 1
}

# Resolve the skia-binaries archive URL for the configuration the workspace
# pins by reading the URL skia-bindings recorded in its build output. Cargo is
# asked which build directory that is: target/ keeps the output of every
# skia-bindings version and feature set ever built there and cargo never prunes
# it, so searching it directly turns up archives for configurations the
# workspace stopped using long ago.
resolve_archive_url() {
    messages=$(mktemp)
    trap 'rm -f "$messages"' EXIT

    # Build script output is reported even when the script was not re-run, so
    # an already built workspace answers this without recompiling anything.
    if ! cargo build --manifest-path "$REPO_ROOT/Cargo.toml" \
        --message-format=json >"$messages"; then
        echo "error: the workspace does not build, so cargo cannot say which" >&2
        echo "       archive skia-bindings needs." >&2
        exit 1
    fi

    out_dirs=$(jq -r 'select(.reason == "build-script-executed")
                      | select(.package_id | test("(^|[#/])skia-bindings[@ ]"))
                      | .out_dir' "$messages" | sort -u)

    if [ -z "$out_dirs" ]; then
        echo "error: cargo reported no skia-bindings build script output, so" >&2
        echo "       the workspace no longer depends on skia-bindings." >&2
        exit 1
    fi

    # OUT_DIR is <build directory>/out; the captured stdout is its sibling.
    urls=$(printf '%s\n' "$out_dirs" | while IFS= read -r out_dir; do
        grep -hE '^[[:space:]]*FROM:' "${out_dir%/out}/output" || true
    done | sed -E 's/^[[:space:]]*FROM:[[:space:]]*//' | sort -u)

    if [ "$(printf '%s\n' "$urls" | grep -c .)" -ne 1 ]; then
        echo "error: skia-bindings did not record exactly one archive URL for" >&2
        echo "       the pinned configuration:" >&2
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

# Resolved into a variable first: a command substitution swallows the exit
# status of what it runs, so inlining the call would carry on with an empty URL
# after resolution has already reported why it failed.
archive_url=$(resolve_archive_url)
obtain_prebuilt "$archive_url"
