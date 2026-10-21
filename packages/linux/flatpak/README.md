# Flatpak

Flatpak packages of the SoulCrafted Slicer: one for the GUI, one for the
command-line tool.

## Building

```
./build.sh
```

It leaves one bundle per application id under `.cache/`, named
`<application id>-<version>-<arch>.flatpak`, where `<arch>` is the architecture
of the build machine as Flatpak names it, such as `x86_64` or `aarch64`. An
unchanged source reuses the cached build. If a vendored source declaration has
gone stale, it stops before doing any work and names the script to run; see
[Vendored sources](#vendored-sources).

## Installing

```
flatpak install --user .cache/it.soulcrafted.slicer-<version>-<arch>.flatpak
flatpak install --user .cache/it.soulcrafted.slicer.cli-<version>-<arch>.flatpak
```

## Running

The GUI is launched from its desktop entry, or by application id:

```
flatpak run it.soulcrafted.slicer
```

The command-line tool needs the Flatpak export directory on `PATH`, which is
not the default on every distribution:

```
export PATH="$PATH:/var/lib/flatpak/exports/bin"              # system install
export PATH="$PATH:$HOME/.local/share/flatpak/exports/bin"    # --user install
```

Its command is named after the application id:

```
it.soulcrafted.slicer.cli --help
```

The wrapper Flatpak generates forwards arguments to the tool rather than to
`flatpak run`, so a filesystem grant cannot be passed through it. The package
declares `--filesystem=host` instead; narrow it with `flatpak override`.

The GUI package carries the same binary, reachable without the `PATH` entry.
That route grants no filesystem access, so pass a path when one is needed:

```
flatpak run --filesystem=home --command=sc-slicer-cli it.soulcrafted.slicer ...
```

## Vendored sources

flatpak-builder compiles with no network access, so every crate the workspace
pulls and the prebuilt Skia archive `skia-bindings` links are declared up front
in two committed files, `cargo-sources.json` and `skia-binaries.source.json`.

Noticing when those go stale is not your job: `build.sh` derives what they
should hold from `Cargo.lock` and stops, naming the one to renew. Run what it
names, then build again.

```
./vendor-cargo-sources.sh    # needs uv
./vendor-skia-binaries.sh    # needs jq and a workspace that compiles
```

It stops rather than renewing anything itself, because renewing the Skia
declaration compiles the whole workspace.

One case escapes the check. The Skia archive name also encodes the
`skia-bindings` feature set, which cannot be derived without compiling
`skia-bindings`, so a feature change has to be caught by hand. Only the GUI
package uses that declaration; the command-line package compiles no Skia.

## Files

- `it.soulcrafted.slicer.yml`, the GUI manifest.
- `it.soulcrafted.slicer.cli.yml`, the command-line manifest. The tool is
  packaged under an application id of its own because Flatpak exports a single
  command per id, named after that id. It builds only `sc-slicer-cli`, so it
  needs neither Slint nor Skia.
- `cargo-sources.json` and `skia-binaries.source.json`, the vendored source
  declarations, with the `vendor-*.sh` scripts that renew them and
  `check-vendored-sources.sh` that decides when they need it.
- `build.sh`.

The desktop entry, the AppStream metainfo template and the icon live under
[`../../shared/`](../../shared/README.md).

Everything `build.sh` generates stays under `.cache/` and is never committed:
the source tarball, the metainfo rendered for the version being built, and a
source list declaring each of them with its checksum.
