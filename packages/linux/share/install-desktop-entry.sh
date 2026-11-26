#!/bin/sh
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
#
# Adds Breadslicer from this unpacked folder to your desktop's menu, with its
# icon and its file types (.bread, .stl, .obj, .3mf), for your account only:
# everything goes under ~/.local/share. Run it again after moving the
# folder. `--remove` takes it all out again.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
id=it.soulcrafted.slicer

if [ "${1:-}" = "--remove" ]; then
    rm -f "$data/applications/$id.desktop" "$data/mime/packages/$id.xml"
    find "$data/icons/hicolor" -name "$id.png" -delete 2>/dev/null || true
else
    mkdir -p "$data/applications" "$data/mime/packages"
    sed "s|^Exec=breadslicer|Exec=\"$here/bin/breadslicer\"|" \
        "$here/share/applications/$id.desktop" > "$data/applications/$id.desktop"
    cp "$here/share/mime/packages/$id.xml" "$data/mime/packages/"
    for icon in "$here"/share/icons/hicolor/*/apps/$id.png; do
        size="$(basename "$(dirname "$(dirname "$icon")")")"
        mkdir -p "$data/icons/hicolor/$size/apps"
        cp "$icon" "$data/icons/hicolor/$size/apps/"
    done
fi

command -v update-mime-database >/dev/null && update-mime-database "$data/mime" || true
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
echo "done"
