# Flatpak

Flatpak package of the SoulCrafted Slicer.

## Files

- `it.soulcrafted.slicer.yml` is the GUI application manifest.
- `it.soulcrafted.slicer.cli.yml` is the command-line application manifest.
  Flatpak exports one command per application id, named after that id, so the
  command-line tool is packaged under an id of its own to reach the host PATH.
  It builds only `sc-slicer-cli`, so it needs neither Slint nor Skia.
- `cargo-sources.json` and `skia-binaries.source.json` are the offline build
  inputs: every crate the workspace pulls, and the prebuilt Skia archive that
  `skia-bindings` links. Flatpak builds run with no network access, so both are
  fetched up front from these declarations.
- `vendor-cargo-sources.sh` regenerates `cargo-sources.json` from `Cargo.lock`.
- `vendor-skia-binaries.sh` regenerates `skia-binaries.source.json`.
- `build.sh` packs the working tree into a cacheable archive and builds the
  package from it into `.cache/build/`. It also generates
  `.cache/source.source.json`, the source list that declares that archive and
  its checksum; flatpak-builder keys its build cache on that checksum, so the
  file has to be regenerated alongside the archive and is never committed.

The `.desktop` entry and AppStream metainfo the manifest installs live under
`../../shared/`; the icon is reused from `apps/slicer-gui/assets/`.

## Building

Run the build script. It packs the working tree (without the multi-gigabyte
`target/`) into a deterministic tarball and builds both packages from it,
leaving one bundle per application id under `.cache/`. An unchanged source
reuses the cached build:

```
./build.sh
```

Install the bundles separately:

```
flatpak install --user .cache/it.soulcrafted.slicer.flatpak
flatpak install --user .cache/it.soulcrafted.slicer.cli.flatpak
```

## Running the command-line tool

Installing the CLI package puts a command on the host PATH, named after the
application id:

```
it.soulcrafted.slicer.cli --help
```

This requires the Flatpak export directory to be on PATH, which is not the
default on every distribution. Add whichever installation was used:

```
export PATH="$PATH:/var/lib/flatpak/exports/bin"              # system install
export PATH="$PATH:$HOME/.local/share/flatpak/exports/bin"    # --user install
```

That wrapper forwards its arguments to the tool rather than to `flatpak run`,
so a filesystem grant cannot be passed through it and the package declares
`--filesystem=host` instead. Use `flatpak override` to narrow it.

The GUI package also carries the binary, reachable without the PATH entry:

```
flatpak run --command=sc-slicer-cli it.soulcrafted.slicer --help
```

That route grants no filesystem access, so pass a path when needed:

```
flatpak run --filesystem=home --command=sc-slicer-cli it.soulcrafted.slicer ...
```

## Regenerating the offline sources

Run after the dependency graph changes:

```
./vendor-cargo-sources.sh    # after Cargo.lock changes
./vendor-skia-binaries.sh    # after the skia-bindings version or features change
```
