#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
#
# Builds the release binaries and the portable Linux packages, inside the
# portable image (docker/portable.Dockerfile). Run by ../build.sh; expects the
# source in /src, the target folder in CARGO_TARGET_DIR and writes to /out:
#
#   Breadslicer-<version>-linux-x86_64.tar.gz   binaries + data, /usr layout
#   Breadslicer-<version>-x86_64.AppImage       the same folder as an AppImage
#   PKGBUILD                                    Arch package from the tar.gz
set -euo pipefail

SRC=/src
OUT=/out
HERE="$SRC/packages/linux"
TARGET="${CARGO_TARGET_DIR:-$SRC/target}"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/apps/slicer-gui/Cargo.toml" | head -n1)
name="Breadslicer-$version-linux-x86_64"
echo "== release build, version $version"

cd "$SRC"
start=$(date +%s)
# The UI crate alone takes ~6 GB to compile: first and alone (see test.sh).
cargo build --release --locked -p bread-gui-ui
cargo build --release --locked -p sc-slicer-gui -p sc-slicer-cli --bins
echo "== release build took $(( $(date +%s) - start )) s"

work=$(mktemp -d)
stage="$work/$name"
mkdir -p "$stage/bin" "$stage/share/soulcrafted-slicer" "$stage/share/applications" \
         "$stage/share/mime/packages" "$stage/share/licenses/breadslicer"

install -m755 "$TARGET/release/breadslicer" "$TARGET/release/sc-slicer-cli" "$stage/bin/"
cp -r "$SRC/assets/profiles" "$stage/share/soulcrafted-slicer/profiles"
install -m644 "$SRC/packages/shared/it.soulcrafted.slicer.desktop" "$stage/share/applications/"
install -m644 "$HERE/share/it.soulcrafted.slicer.xml" "$stage/share/mime/packages/"
for f in LICENSE.md NOTICE; do
    install -m644 "$SRC/$f" "$stage/$f"
    install -m644 "$SRC/$f" "$stage/share/licenses/breadslicer/$f"
done
install -m644 "$SRC/libs/project-formats/NOTICE.bread-format" "$stage/NOTICE.bread-format"
install -m644 "$SRC/libs/project-formats/NOTICE.bread-format" "$stage/share/licenses/breadslicer/"
sed "s/{version}/$version/g" "$HERE/share/LEEME.txt" > "$stage/LEEME.txt"
install -m755 "$HERE/share/install-desktop-entry.sh" "$stage/install-desktop-entry.sh"

# The icon in every size the Windows icon holds, from the same file.
python3 - "$SRC/apps/slicer-gui/assets/icons/breadslicer.ico" "$stage/share/icons/hicolor" <<'PY'
import os, sys
from PIL import Image
ico, root = sys.argv[1], sys.argv[2]
img = Image.open(ico)
for size in sorted(img.info.get("sizes", {img.size})):
    img.size = size
    frame = img.convert("RGBA")
    out = os.path.join(root, f"{size[0]}x{size[1]}", "apps")
    os.makedirs(out, exist_ok=True)
    frame.save(os.path.join(out, "it.soulcrafted.slicer.png"))
    print("icon", size)
PY

desktop-file-validate "$stage/share/applications/it.soulcrafted.slicer.desktop"

(cd "$stage" && find . -type f ! -name SHA256SUMS.txt -print0 | sort -z \
    | xargs -0 sha256sum | sed 's|  \./|  |' > SHA256SUMS.txt)

mkdir -p "$OUT"
tarball="$OUT/$name.tar.gz"
tar -C "$work" --owner=0 --group=0 -czf "$tarball" "$name"
echo "== $tarball"

# AppImage: the same folder as AppDir/usr.
appdir="$work/AppDir"
mkdir -p "$appdir/usr"
cp -r "$stage/bin" "$stage/share" "$appdir/usr/"
cp "$stage/LICENSE.md" "$stage/NOTICE" "$stage/NOTICE.bread-format" "$appdir/"
cp "$stage/share/applications/it.soulcrafted.slicer.desktop" "$appdir/"
cp "$stage/share/icons/hicolor/256x256/apps/it.soulcrafted.slicer.png" "$appdir/"
ln -s it.soulcrafted.slicer.png "$appdir/.DirIcon"
cat > "$appdir/AppRun" <<'SH'
#!/bin/sh
# The program reads its data from ../share beside its bin folder.
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/breadslicer" "$@"
SH
chmod +x "$appdir/AppRun"
appimage="$OUT/Breadslicer-$version-x86_64.AppImage"
ARCH=x86_64 /opt/appimage/appimagetool --no-appstream \
    --runtime-file /opt/appimage/runtime-x86_64 "$appdir" "$appimage"
echo "== $appimage"

sum=$(sha256sum "$tarball" | cut -d' ' -f1)
sed -e "s/@VERSION@/$version/g" -e "s/@SHA256@/$sum/g" \
    "$HERE/arch/PKGBUILD.in" > "$OUT/PKGBUILD"
echo "== $OUT/PKGBUILD"

echo "== shared libraries the GUI needs"
readelf -d "$stage/bin/breadslicer" | grep NEEDED || true
echo "== newest glibc symbol"
objdump -T "$stage/bin/breadslicer" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1
rm -rf "$work"
