#!/bin/sh

# Report whether the vendored source declarations still match Cargo.lock.
#
# Both declarations are derived from the lock file, so staleness is decidable
# from committed files alone, with nothing recorded on the side:
#
#   cargo-sources.json declares one archive per registry package, unpacked at
#   cargo/vendor/<name>-<version>. That set has to be the set of registry
#   packages in Cargo.lock.
#
#   skia-binaries.source.json points at a rust-skia release whose tag is the
#   skia-bindings version, so the release segment of the URL has to be the
#   locked skia-bindings version.
#
# What this cannot see: the archive filename also encodes the skia-bindings
# feature set and a hash of its build configuration. Deriving the expected
# values needs skia-bindings itself, which means compiling the workspace, so a
# feature change is not caught here.
#
# Exits non-zero when a declaration is stale, naming the script that renews it.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
LOCK="$REPO_ROOT/Cargo.lock"
CARGO_SOURCES="$SCRIPT_DIR/cargo-sources.json"
SKIA_SOURCE="$SCRIPT_DIR/skia-binaries.source.json"

if ! command -v jq >/dev/null 2>&1; then
    echo "error: jq is required to read the vendored source declarations." >&2
    exit 1
fi

for path in "$LOCK" "$CARGO_SOURCES" "$SKIA_SOURCE"; do
    if [ ! -f "$path" ]; then
        echo "check-vendored-sources: missing file: $path" >&2
        exit 1
    fi
done

# Every field of a lock entry, gathered until the blank line that ends it.
# Packages built from a path carry no source key and are not vendored.
lock_vendor_dests() {
    awk '
        /^\[\[package\]\]/ { name = ""; version = ""; registry = 0; next }
        /^name = "/    { name = $0;    sub(/^name = "/, "", name);       sub(/"$/, "", name) }
        /^version = "/ { version = $0; sub(/^version = "/, "", version); sub(/"$/, "", version) }
        /^source = "registry\+/ { registry = 1 }
        /^[[:space:]]*$/ {
            if (registry && name != "" && version != "") print "cargo/vendor/" name "-" version
            name = ""; version = ""; registry = 0
        }
        END {
            if (registry && name != "" && version != "") print "cargo/vendor/" name "-" version
        }
    ' "$LOCK" | sort
}

lock_package_version() {
    awk -v wanted="$1" '
        /^\[\[package\]\]/ { name = ""; version = ""; next }
        /^name = "/    { name = $0;    sub(/^name = "/, "", name);       sub(/"$/, "", name) }
        /^version = "/ { version = $0; sub(/^version = "/, "", version); sub(/"$/, "", version) }
        name == wanted && version != "" { print version; exit }
    ' "$LOCK"
}

stale=0

locked_dests=$(lock_vendor_dests)
declared_dests=$(jq -r '.[] | select(.type == "archive") | .dest' "$CARGO_SOURCES" | sort)

if [ "$locked_dests" != "$declared_dests" ]; then
    locked_file=$(mktemp)
    declared_file=$(mktemp)
    printf '%s\n' "$locked_dests" > "$locked_file"
    printf '%s\n' "$declared_dests" > "$declared_file"

    echo "check-vendored-sources: cargo-sources.json does not match Cargo.lock" >&2
    comm -23 "$locked_file" "$declared_file" | sed 's|^cargo/vendor/|  locked but not declared: |' >&2
    comm -13 "$locked_file" "$declared_file" | sed 's|^cargo/vendor/|  declared but not locked: |' >&2
    echo "  Run ./vendor-cargo-sources.sh" >&2

    rm -f "$locked_file" "$declared_file"
    stale=1
fi

declared_release=$(jq -r '.[0].url' "$SKIA_SOURCE" \
    | sed -n 's|.*/releases/download/\([^/]*\)/.*|\1|p')
locked_skia=$(lock_package_version skia-bindings)

if [ -z "$declared_release" ] || [ -z "$locked_skia" ]; then
    echo "check-vendored-sources: could not read the skia-bindings version from" >&2
    echo "  skia-binaries.source.json or from Cargo.lock" >&2
    stale=1
elif [ "$declared_release" != "$locked_skia" ]; then
    echo "check-vendored-sources: skia-binaries.source.json is stale" >&2
    echo "  it declares the archive from release $declared_release" >&2
    echo "  Cargo.lock pins skia-bindings $locked_skia" >&2
    echo "  Run ./vendor-skia-binaries.sh" >&2
    stale=1
fi

if [ "$stale" -eq 0 ]; then
    echo "check-vendored-sources: cargo-sources.json and skia-binaries.source.json are current"
fi

exit "$stale"
