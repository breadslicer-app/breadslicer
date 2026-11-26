#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
#
# Tries the packages /out holds, inside the Arch image: builds the Arch
# package from the PKGBUILD and the .tar.gz, installs it, starts the app
# under Xvfb with a model, hands it a second model from a second launch,
# then does the same with the AppImage. Screenshots and logs go to /out.
set -euo pipefail

version="${BREADSLICER_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' /src/apps/slicer-gui/Cargo.toml | head -n1)}"
models=/src/libs/mesh-formats/tests/fixtures/official/burkardt-stla

echo "== makepkg"
work=$(mktemp -d)
cp /out/PKGBUILD "/out/Breadslicer-$version-linux-x86_64.tar.gz" "$work/"
chown -R builder "$work"
su builder -c "cd $work && makepkg -f --nodeps" >/out/smoke-makepkg.log 2>&1
pacman -U --noconfirm "$work"/breadslicer-bin-*.pkg.tar.zst >/dev/null
pacman -Ql breadslicer-bin | grep -E 'bin/|\.desktop|mime/packages|256x256|profiles/$'

export XDG_RUNTIME_DIR=/tmp/runtime
mkdir -p -m700 "$XDG_RUNTIME_DIR"
Xvfb :99 -screen 0 1600x1000x24 >/dev/null 2>&1 &
xvfb=$!
export DISPLAY=:99
# The container has no GPU: Slint takes Mesa's software Vulkan only when asked.
export SLINT_WGPU_CPU=1
sleep 2

# Starts `$2...` as the app, waits for its window, takes `/out/$1.png`.
try_app() {
    local shot="$1"
    shift
    "$@" "$models/teapot.stl" >"/out/$shot.log" 2>&1 &
    local app=$!
    local i tree line=""
    for i in $(seq 1 180); do
        tree=$(xwininfo -root -tree 2>/dev/null || true)
        line=$(printf '%s\n' "$tree" | grep -m1 -E '"([^"]* )?Breadslicer"' || true)
        if [ -n "$line" ]; then echo "window after $i s"; break; fi
        if ! kill -0 "$app" 2>/dev/null; then echo "!! the app ended"; tail -n 30 "/out/$shot.log"; return 1; fi
        sleep 1
    done
    if [ -z "$line" ]; then
        echo "!! no Breadslicer window after 180 s; windows:"
        printf '%s\n' "$tree" | grep '"' | head -n 20
        import -window root "/out/$shot.png" || true
        tail -n 30 "/out/$shot.log"
        kill "$app" 2>/dev/null || true
        return 1
    fi
    sleep 8
    local win
    win=$(printf '%s\n' "$line" | awk '{print $1}')
    echo "window: $(printf '%s\n' "$line" | sed 's/^ *//')"
    xprop -id "$win" WM_CLASS _NET_WM_NAME 2>/dev/null || true
    # A second launch with another model hands it over and ends.
    local start=$SECONDS
    if timeout 20 "$@" "$models/sphere.stl" >"/out/$shot-second.log" 2>&1; then
        echo "second launch ended after $(( SECONDS - start )) s (handed over)"
    else
        echo "!! second launch did not end by itself"
    fi
    sleep 4
    import -window root "/out/$shot.png"
    kill "$app" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    echo "screenshot: /out/$shot.png"
}

echo "== installed package"
try_app smoke-pkg breadslicer || failed=1

echo "== AppImage"
chmod +x "/out/Breadslicer-$version-x86_64.AppImage"
APPIMAGE_EXTRACT_AND_RUN=1 try_app smoke-appimage "/out/Breadslicer-$version-x86_64.AppImage" || failed=1

kill "$xvfb" 2>/dev/null || true
exit "${failed:-0}"
