# macOS packaging

Packaging of the SoulCrafted Slicer for macOS on Apple Silicon (arm64): a
`.app` bundle holding both binaries, delivered in a DMG the user installs by
dragging the bundle to `/Applications`.

## Files

- `Info.plist` is the bundle manifest. `build.sh` adds the version keys to its
  staged copy, taking the number from the crate.
- `it.soulcrafted.slicer.icns` is generated from the shared SVG logo. It serves
  as the bundle icon and as the mounted volume's icon.
- `build.sh` builds both release binaries, stages the bundle, signs it and
  wraps it in a DMG. Everything lands under `.cache`; nothing is installed.

## Installed layout

```
SoulCrafted Slicer.app/Contents/
  Info.plist
  MacOS/breadslicer           the bundle's main executable
  MacOS/sc-slicer-cli        the command-line tool
  Resources/it.soulcrafted.slicer.icns
  Resources/slicer/profiles/ the bundled profiles
```

The command-line tool is reachable at
`/Applications/SoulCrafted Slicer.app/Contents/MacOS/sc-slicer-cli`. A `.app`
cannot put anything on the `PATH`; a Homebrew formula is the intended answer.

## Prerequisites

- The Xcode Command Line Tools (`xcode-select --install`), which provide
  `codesign`, `hdiutil`, `plutil`, `otool`, `notarytool` and `stapler`. A full
  Xcode install is not needed.
- The Rust `aarch64-apple-darwin` target.
- To build for distribution: a "Developer ID Application" certificate in the
  keychain, which requires Apple Developer Program membership.

## Building

```
sh ./build.sh
```

It emits `sc-slicer-<version>-arm64.dmg` under `.cache`.

The build refuses to package when the icon is older than the logo, when
`Info.plist` declares a lower minimum macOS version than `rustc` builds for,
or when either binary links a library outside the system paths.

## Signing and notarization

`build.sh` signs with the first "Developer ID Application" identity in the
keychain, adding the hardened runtime and a secure timestamp. Without one it
signs ad hoc, still with the hardened runtime. `CODESIGN_IDENTITY` names an
identity instead; set it to `-` to force an ad hoc signature.

Notarization needs a Developer ID identity and `NOTARY_PROFILE`. Store the
credentials once, then name the profile per build:

```
xcrun notarytool store-credentials "soulcrafted" \
    --apple-id "<apple-id>" \
    --team-id "<team-id>" \
    --password "<app-specific-password>"
NOTARY_PROFILE=soulcrafted sh ./build.sh
```

`build.sh` submits and staples the DMG, which tickets the bundle inside it too.

An ad hoc image is not distributable. Gatekeeper refuses it once it carries a
download's quarantine flag, leaving the user to allow it by hand under Privacy
and Security in System Settings.

## Regenerating the icon

Run after the logo changes, and commit the result:

```
cargo run -p sc-icon-gen
```
