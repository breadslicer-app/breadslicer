#!/usr/bin/env sh
# Regenerate the small gallery thumbnails used by the Preferences printer
# picker. Each printer image under assets/profiles/images/ is downscaled to
# fit within a square bounding box and written, under the same file name, to
# the sibling thumbnails/ directory. The gallery prefers a thumbnail when one
# exists and falls back to the full-size image otherwise, so this step is an
# optimisation, not a hard dependency.
#
# Requires ImageMagick (the `magick` command).
set -eu

# Longest-edge size in pixels. The tiles render the image at ~96px tall, so
# 256px covers high-DPI displays with room to spare while keeping the decoded
# texture small.
MAX_EDGE=256

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../../.." && pwd)
images_dir="$repo_root/assets/profiles/images"
thumbs_dir="$images_dir/thumbnails"

mkdir -p "$thumbs_dir"

count=0
for src in "$images_dir"/*.png; do
	[ -e "$src" ] || continue
	name=$(basename -- "$src")
	magick "$src" -strip -resize "${MAX_EDGE}x${MAX_EDGE}>" "$thumbs_dir/$name"
	count=$((count + 1))
done

echo "wrote $count thumbnails to $thumbs_dir"
