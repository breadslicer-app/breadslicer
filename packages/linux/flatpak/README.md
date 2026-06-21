# Flatpak

Flatpak package of the SoulCrafted Slicer.

## Files

- `it.soulcrafted.Slicer.yml` is the application manifest.
- `cargo-sources.json` and `skia-binaries.source.json` are the offline build
  inputs: every crate the workspace pulls, and the prebuilt Skia archive that
  `skia-bindings` links. Flatpak builds run with no network access, so both are
  fetched up front from these declarations.
- `vendor-cargo-sources.sh` regenerates `cargo-sources.json` from `Cargo.lock`.
- `vendor-skia-binaries.sh` regenerates `skia-binaries.source.json`.
- `build.sh` packs the working tree into a cacheable archive and builds the
  package from it into `.cache/build/`.

The `.desktop` entry and AppStream metainfo the manifest installs live under
`../../shared/`; the icon is reused from `apps/slicer-gui/assets/`.

## Building

Run the build script. It packs the working tree (without the multi-gigabyte
`target/`) into a deterministic tarball and builds the Flatpak from it, leaving
the result under `.cache/build/`. An unchanged source reuses the cached build:

```
./build.sh
```

The command-line tool ships in the same package:

```
flatpak run --command=sc-slicer-cli it.soulcrafted.Slicer --help
```

It reads and writes files through the sandbox, so grant it a path when needed,
for example:

```
flatpak run --filesystem=home --command=sc-slicer-cli it.soulcrafted.Slicer ...
```

## Regenerating the offline sources

Run after the dependency graph changes:

```
./vendor-cargo-sources.sh    # after Cargo.lock changes
./vendor-skia-binaries.sh    # after the skia-bindings version or features change
```
