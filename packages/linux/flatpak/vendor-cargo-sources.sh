#!/bin/sh

# Regenerate cargo-sources.json: the Flatpak source list for every crate the
# workspace depends on, so an offline Flatpak build (which has no network
# access) can run `cargo build --offline`. Run this whenever Cargo.lock changes.
#
# It downloads a pinned revision of the upstream flatpak-cargo-generator,
# verifies its checksum, and runs it against the workspace Cargo.lock. The
# generator declares its own Python dependencies inline (PEP 723), so it runs
# through uv, which provisions them in an isolated, throwaway environment.

set -eu

# Pinned upstream generator (flatpak/flatpak-builder-tools). Bump the commit
# and the checksum together to update it.
GENERATOR_COMMIT="737c0085912f9f7dabf9341d4608e2a77a51a73a"
GENERATOR_SHA256="b373c8ab1a05378ec5d8ed0645c7b127bcec7d2f7a1798694fbc627d570d856c"
GENERATOR_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$GENERATOR_COMMIT/cargo/flatpak-cargo-generator.py"

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
CACHE_DIR="$SCRIPT_DIR/.cache"
GENERATOR="$CACHE_DIR/flatpak-cargo-generator-$GENERATOR_COMMIT.py"
SOURCE_FILE="$SCRIPT_DIR/cargo-sources.json"

if ! command -v uv >/dev/null 2>&1; then
    echo "error: uv is required to run the generator (it provisions the" >&2
    echo "       generator's Python dependencies in a throwaway environment)." >&2
    echo "       Install uv: https://docs.astral.sh/uv/" >&2
    exit 1
fi

# True when $1 has the expected sha256 in $2.
verify_sha256() {
    [ "$(sha256sum "$1" | cut -d ' ' -f 1)" = "$2" ]
}

mkdir -p "$CACHE_DIR"
if [ ! -f "$GENERATOR" ] || ! verify_sha256 "$GENERATOR" "$GENERATOR_SHA256"; then
    curl --proto '=https' --tlsv1.2 -fL --retry 2 -o "$GENERATOR.tmp" "$GENERATOR_URL"
    if ! verify_sha256 "$GENERATOR.tmp" "$GENERATOR_SHA256"; then
        rm -f "$GENERATOR.tmp"
        echo "error: the downloaded generator does not match the pinned" >&2
        echo "       checksum ($GENERATOR_SHA256)." >&2
        exit 1
    fi
    mv -f "$GENERATOR.tmp" "$GENERATOR"
fi

uv run --no-project "$GENERATOR" "$REPO_ROOT/Cargo.lock" -o "$SOURCE_FILE"

echo "Wrote $SOURCE_FILE"
