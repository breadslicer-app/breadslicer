#!/bin/sh

# Render the AppStream metainfo that ships in a package.
#
# usage: render-metainfo.sh <template> <crate-manifest> <output>
#
# The template under packages/shared carries everything that does not change
# between builds and deliberately holds no <releases> block. This adds one,
# naming the version of the crate being packaged and the date of the build.
# Keeping the release out of the tracked file is what stops it drifting from
# the crate version: there is no second copy of the number to fall behind.
#
# AppStream requires at least day-level granularity for the date and expects
# releases newest first, so a single entry rendered per build satisfies both.

set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: render-metainfo.sh <template> <crate-manifest> <output>" >&2
    exit 2
fi

template=$1
manifest=$2
output=$3

for path in "$template" "$manifest"; do
    if [ ! -f "$path" ]; then
        echo "render-metainfo: missing file: $path" >&2
        exit 1
    fi
done

if grep -q '<releases' "$template"; then
    echo "render-metainfo: $template already carries a <releases> block." >&2
    echo "  The release is rendered per build, so the template must not pin" >&2
    echo "  one; remove it to keep the version from drifting." >&2
    exit 1
fi

if ! grep -q '</component>' "$template"; then
    echo "render-metainfo: no </component> in $template" >&2
    exit 1
fi

# The version from the [package] section, ignoring any other section that also
# carries a version key.
version=$(awk '
    /^[[:space:]]*\[/ { in_package = ($0 ~ /^[[:space:]]*\[package\][[:space:]]*$/); next }
    in_package && /^[[:space:]]*version[[:space:]]*=/ {
        if (match($0, /"[^"]*"/)) {
            print substr($0, RSTART + 1, RLENGTH - 2)
            exit
        }
    }
' "$manifest")

if [ -z "$version" ]; then
    echo "render-metainfo: no [package] version found in $manifest" >&2
    exit 1
fi

released=$(date -u +%Y-%m-%d)

awk -v version="$version" -v released="$released" '
    /<\/component>/ && !inserted {
        print "  <releases>"
        printf "    <release version=\"%s\" date=\"%s\"/>\n", version, released
        print "  </releases>"
        inserted = 1
    }
    { print }
' "$template" > "$output"

echo "render-metainfo: wrote $output for version $version dated $released"
