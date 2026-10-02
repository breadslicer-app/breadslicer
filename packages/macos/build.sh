#!/bin/sh

# Build the SoulCrafted Slicer macOS application bundle and its disk image.
#
# Stages a .app carrying both binaries, the icon and the bundled profiles,
# signs it, then wraps it in a DMG next to a symlink to /Applications so the
# user installs by dragging. Everything lands under .cache beside this script;
# nothing is installed.
#
# Signing degrades on purpose. With a "Developer ID Application" identity in
# the keychain the bundle is signed with the hardened runtime and a secure
# timestamp, which is what the notary service requires. Without one it is
# signed ad hoc, still with the hardened runtime, so a build without a
# certificate exercises the same runtime restrictions the released one will.
# An ad hoc bundle runs on the machine that built it; Gatekeeper rejects it
# once it carries a download's quarantine flag.
#
# Notarization runs only when both an identity and a notarytool keychain
# profile are available. See README.md.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
CACHE="$SCRIPT_DIR/.cache"
TARGET=aarch64-apple-darwin
RELEASE_DIR="$REPO_ROOT/target/$TARGET/release"
APP_NAME="SoulCrafted Slicer"
BUNDLE_ID=it.soulcrafted.slicer
GUI_BIN=sc-slicer-gui
CLI_BIN=sc-slicer-cli
ICNS_NAME="$BUNDLE_ID.icns"
SVG_RELPATH="packages/shared/$BUNDLE_ID.svg"
ICNS_RELPATH="packages/macos/$ICNS_NAME"

# Set to a signing identity to override the one found in the keychain, or to
# `-` to force an ad hoc signature even when a Developer ID is installed.
CODESIGN_IDENTITY=${CODESIGN_IDENTITY:-}
# Name of a profile stored with `xcrun notarytool store-credentials`. Empty
# skips notarization.
NOTARY_PROFILE=${NOTARY_PROFILE:-}

fail() {
    echo "macos/build: $1" >&2
    exit 1
}

# --- Checks ------------------------------------------------------------

commit_time() {
    git -C "$REPO_ROOT" log -1 --no-show-signature --format=%ct -- "$1" 2>/dev/null || true
}

has_uncommitted_change() {
    [ -n "$(git -C "$REPO_ROOT" status --porcelain -- "$1" 2>/dev/null)" ]
}

# The icon is generated from the shared logo by `cargo run -p sc-icon-gen` and
# committed, with nothing tying the copy back to its source. Catch the case
# where the logo moved on and the icon was never regenerated.
#
# Ordering comes from the commit timestamps rather than the files' own
# modification times. A checkout stamps every file with the time it was
# written, which says nothing about which was edited first, so a file-time
# comparison reports whichever way the checkout happened to land.
assert_icon_current() {
    for icon_file in "$SVG_RELPATH" "$ICNS_RELPATH"; do
        [ -f "$REPO_ROOT/$icon_file" ] || fail "expected icon file not found: $icon_file"
    done

    # An edit sitting in the working tree has no commit time yet, so the
    # comparison below cannot see it. A modified logo alongside an untouched
    # icon is the drift this guards against, committed or not.
    if has_uncommitted_change "$SVG_RELPATH" && ! has_uncommitted_change "$ICNS_RELPATH"; then
        fail "$SVG_RELPATH has uncommitted changes but $ICNS_RELPATH does not.
  Regenerate the icon with \`cargo run -p sc-icon-gen\`, commit the result,
  then re-run this script."
    fi

    svg_time=$(commit_time "$SVG_RELPATH")
    icns_time=$(commit_time "$ICNS_RELPATH")
    if [ -z "$svg_time" ] || [ -z "$icns_time" ]; then
        echo "Warning: no commit times for the icon files; skipping the freshness check."
        return 0
    fi
    if [ "$icns_time" -lt "$svg_time" ]; then
        fail "$ICNS_RELPATH was last committed before $SVG_RELPATH.
  Regenerate the icon with \`cargo run -p sc-icon-gen\`, commit the result,
  then re-run this script."
    fi
}

# LSMinimumSystemVersion must not promise support below the floor the binaries
# are actually built for. That floor belongs to the toolchain, so read it back
# from rustc instead of trusting the 2 to stay in step by hand.
assert_deployment_target() {
    plist_min=$(plutil -extract LSMinimumSystemVersion raw -o - "$SCRIPT_DIR/Info.plist")
    rustc_min=$(rustc --print deployment-target --target "$TARGET" | sed 's/.*=//')
    if [ "$plist_min" != "$rustc_min" ]; then
        fail "rustc builds $TARGET for macOS $rustc_min but Info.plist declares
  $plist_min. Reconcile the 2, then re-run."
    fi
}

# The icon lookup fails silently: a name that resolves to nothing leaves the
# bundle wearing the generic application icon, with no warning anywhere. That
# the file sits in Resources and the key names it are not enough, because
# CoreFoundation appends `.icns` only when the value carries no extension, and
# a reverse-DNS name has dots to mistake for one.
assert_icon_resolves() {
    icon_value=$(plutil -extract CFBundleIconFile raw -o - "$CONTENTS/Info.plist")
    [ -f "$CONTENTS/Resources/$icon_value" ] || fail \
        "CFBundleIconFile is \"$icon_value\" but Contents/Resources/$icon_value is not there.
  Name the icon file in full, suffix included."
    echo "Icon check passed: CFBundleIconFile resolves to $icon_value."
}

# The bundle ships no libraries of its own, so anything the binaries link
# outside the system paths would be missing on a machine that is not this one.
assert_self_contained() {
    for binary in "$CONTENTS/MacOS/$GUI_BIN" "$CONTENTS/MacOS/$CLI_BIN"; do
        non_system=$(otool -L "$binary" | tail -n +2 | awk '{ print $1 }' \
            | grep -v '^/usr/lib/' | grep -v '^/System/Library/' || true)
        if [ -n "$non_system" ]; then
            fail "$(basename "$binary") links libraries outside the system paths:
$non_system"
        fi
    done
    echo "Import check passed: both binaries link only system libraries."
}

# --- Signing -----------------------------------------------------------

# Only a "Developer ID Application" identity is worth picking up: the notary
# service rejects every other kind, so finding one would promise a signature
# that cannot be notarized.
detect_identity() {
    security find-identity -v -p codesigning 2>/dev/null \
        | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' \
        | head -1
}

# A secure timestamp needs a real certificate and reaches out to
# timestamp.apple.com, so an ad hoc signature cannot carry one.
sign() {
    if [ "$IDENTITY" = "-" ]; then
        codesign --force --options runtime --timestamp=none --sign - "$1"
    else
        codesign --force --options runtime --timestamp --sign "$IDENTITY" "$1"
    fi
}

# --- Build -------------------------------------------------------------

VERSION=$(cargo pkgid -p sc-slicer-gui | sed 's/.*[#@]//')
[ -n "$VERSION" ] || fail "could not read the version of sc-slicer-gui"

assert_icon_current
assert_deployment_target

echo "Building SoulCrafted Slicer $VERSION for $TARGET ..."
(cd "$REPO_ROOT" && cargo build --release --target "$TARGET" -p "$GUI_BIN" -p "$CLI_BIN")

# --- Stage the bundle --------------------------------------------------

APP="$CACHE/$APP_NAME.app"
CONTENTS="$APP/Contents"

echo "Staging $APP_NAME.app ..."
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

for binary in "$GUI_BIN" "$CLI_BIN"; do
    [ -f "$RELEASE_DIR/$binary" ] || fail "expected binary not found: $RELEASE_DIR/$binary"
    cp "$RELEASE_DIR/$binary" "$CONTENTS/MacOS/$binary"
done

cp "$SCRIPT_DIR/$ICNS_NAME" "$CONTENTS/Resources/$ICNS_NAME"
printf 'APPL????' > "$CONTENTS/PkgInfo"

cp "$SCRIPT_DIR/Info.plist" "$CONTENTS/Info.plist"
plutil -insert CFBundleShortVersionString -string "$VERSION" "$CONTENTS/Info.plist"
plutil -insert CFBundleVersion -string "$VERSION" "$CONTENTS/Info.plist"
plutil -lint "$CONTENTS/Info.plist" > /dev/null || fail "the staged Info.plist is malformed"

# The runtime resolves its read-only data root as a `slicer` directory in the
# bundle's Resources; the bundled profiles live under it. Resources rather than
# MacOS because the signature seals Contents/ and Apple keeps non-executable
# payload out of the executable directory.
mkdir -p "$CONTENTS/Resources/slicer/profiles"
cp -R "$REPO_ROOT/assets/profiles/." "$CONTENTS/Resources/slicer/profiles/"

assert_icon_resolves
assert_self_contained

# --- Sign the bundle ---------------------------------------------------

if [ -n "$CODESIGN_IDENTITY" ]; then
    IDENTITY=$CODESIGN_IDENTITY
else
    IDENTITY=$(detect_identity)
    [ -n "$IDENTITY" ] || IDENTITY=-
fi

if [ "$IDENTITY" = "-" ]; then
    echo "Signing ad hoc: no Developer ID Application identity found."
else
    echo "Signing with: $IDENTITY"
fi

# Inside out: a nested executable carries its own signature, and the bundle's
# seal is computed over the tree as it stands, so the outer signature has to go
# last. Nothing in the tree may change after it.
sign "$CONTENTS/MacOS/$CLI_BIN"
sign "$APP"

codesign --verify --deep --strict --verbose=2 "$APP"

# --- Build the disk image ----------------------------------------------

echo "Building the disk image ..."
DMG_ROOT="$CACHE/dmg"
DMG="$CACHE/sc-slicer-$VERSION-arm64.dmg"
DMG_RW="$CACHE/dmg-rw.dmg"

# The image is mounted part way through to set the volume icon. Leaving it
# mounted after a failure there would make the next run's mount land on a
# second volume of the same name, so release it however this script ends.
MOUNT_POINT=
trap 'if [ -n "$MOUNT_POINT" ]; then hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null || true; fi' EXIT

rm -rf "$DMG_ROOT"
mkdir -p "$DMG_ROOT"
# ditto rather than cp: it is the copy that preserves bundle metadata, which
# the signature was just computed over.
ditto "$APP" "$DMG_ROOT/$APP_NAME.app"
# hdiutil stores the link itself, so this costs nothing and does not pull in
# the contents of /Applications.
ln -s /Applications "$DMG_ROOT/Applications"
# The Finder reads a mounted volume's icon from this name, and only when the
# volume root also carries the custom-icon flag set below. Without both the
# volume gets the generic disk image icon.
cp "$SCRIPT_DIR/$ICNS_NAME" "$DMG_ROOT/.VolumeIcon.icns"

# The output name derives from a version that changes rarely and .cache is
# never cleaned, so an image from an earlier run is usually sitting under the
# name this run is about to write. Remove it first: then anything found
# afterwards can only be the product of this run.
rm -f "$DMG" "$DMG_RW"

# That custom-icon flag lives in the volume root's FinderInfo, which no
# read-only image will accept a write to. So the image is built writable,
# flagged while mounted, and only then compressed into what ships.
hdiutil create -volname "$APP_NAME" -srcfolder "$DMG_ROOT" -ov -format UDRW "$DMG_RW"

MOUNT_POINT=$(hdiutil attach "$DMG_RW" -nobrowse -noautoopen \
    | awk -F'\t' '/\/Volumes\//{ print $NF }' | head -1)
[ -n "$MOUNT_POINT" ] || fail "could not mount $DMG_RW to set the volume icon"
[ -f "$MOUNT_POINT/.VolumeIcon.icns" ] || fail "the volume icon did not reach $MOUNT_POINT"
SetFile -a C "$MOUNT_POINT"
hdiutil detach "$MOUNT_POINT" -quiet
MOUNT_POINT=

hdiutil convert "$DMG_RW" -format UDZO -o "$DMG"
rm -f "$DMG_RW"

# Apple asks for this before submission: a corrupt image is rejected, and the
# notary service counts the attempt against the daily limit either way.
hdiutil verify "$DMG"

if [ "$IDENTITY" != "-" ]; then
    codesign --force --timestamp --sign "$IDENTITY" "$DMG"
fi

# --- Notarize ----------------------------------------------------------

if [ "$IDENTITY" = "-" ] || [ -z "$NOTARY_PROFILE" ]; then
    echo ""
    echo "Built: $DMG"
    echo ""
    if [ "$IDENTITY" = "-" ]; then
        echo "This image is signed ad hoc and is NOT notarized. It runs on this"
        echo "machine, but once downloaded it carries a quarantine flag and"
        echo "Gatekeeper refuses it; a user then has to allow it by hand under"
        echo "System Settings > Privacy & Security. Notarizing needs a Developer"
        echo "ID Application certificate; see README.md."
    else
        echo "Signed but NOT notarized: set NOTARY_PROFILE to a profile stored"
        echo "with \`xcrun notarytool store-credentials\`; see README.md."
    fi
    exit 0
fi

# The notary service generates a ticket for the image and for every nested
# item, the bundle included, so submitting the image covers both. A ZIP would
# be accepted too but cannot be stapled.
echo "Submitting to the notary service ..."
xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait

echo "Stapling the ticket ..."
xcrun stapler staple "$DMG"
spctl --assess --type open --context context:primary-signature -vv "$DMG"

echo ""
echo "Built and notarized: $DMG"
echo ""
echo "Apple asks that the log be read even on success, since it can carry"
echo "warnings to fix before the next submission:"
echo "  xcrun notarytool log <submission-id> --keychain-profile $NOTARY_PROFILE"
