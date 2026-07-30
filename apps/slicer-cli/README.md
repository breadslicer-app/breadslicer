# SoulCrafted Slicer (Command Line Interface)

> ⚠️ Work in progress

`sc-slicer-cli` is the command line face of the
[SoulCrafted Slicer](../../README.md). It slices meshes for resin printers,
converts between mesh file formats, diagnoses and repairs meshes, and manages
the profile library, all without opening a window.

Every subcommand accepts `--help`:

```
sc-slicer-cli slice --help
```

## Subcommands

| Subcommand | What it does |
|------------|--------------|
| `slice`    | Slice a mesh into per-layer output for a given printer and resin. |
| `convert`  | Convert a mesh file from one format to another. |
| `diagnose` | Report the problems a mesh has, without changing it. |
| `fix`      | Write a repaired copy of a mesh. |
| `profiles` | `list`, `export` and `import` profiles in the user library. |
| `version`  | Print the version. |

## Slicing

Slicing needs a mesh, an output path, a printer profile and a material
profile. Profiles are named by their id, which is looked up in the user
library and then in the profiles the installation ships:

```
sc-slicer-cli slice \
  --source model.stl \
  --target model.slices.3mf \
  --printer-profile elegoo-saturn-4-ultra-ctb \
  --material-profile sirayatech-blu-saturn4
```

A value that looks like a path (it starts with `/` or `./`) is read as a
profile file instead, so an uninstalled profile can be used directly:

```
sc-slicer-cli slice \
  --source model.stl \
  --target ./layers \
  --target-format png-per-layer \
  --printer-profile ./my-printer.toml \
  --material-profile ./my-resin.toml
```

The output format is inferred from the target's extension when it can be
(`.3mf`, `.svg`); per-layer formats write into a directory and have no
extension to infer from, so they need `--target-format`. The accepted values
are `3mf`, `svg-combined`, `svg-per-layer` and `png-per-layer`.

The layer height comes from the resolved material profile. `--layer-height`
and `--first-layer-height` override it in millimetres. `--aa` sets per-pixel
anti-aliasing to 0 (off), 2, 4 or 8; it only affects raster output.

By default the scene is auto-placed: centred on the build plate with its
lowest point at Z = 0. `--no-auto-place` slices at the file's own coordinates
instead. A multi-model scene keeps its relative layout either way.

Slicing refuses a mesh that is not a closed manifold. `--repair` runs the same
repairs as `fix` first; `--verbose` lists every offending face instead of a
summary when validation still fails.

## Converting, diagnosing and repairing

`convert` reads one mesh format and writes another. Formats are inferred from
the file extensions, and `--source-format` / `--target-format` override that:

```
sc-slicer-cli convert --source model.obj --target model.3mf
```

`diagnose` reports what is wrong with a mesh and leaves it alone. `fix`
writes a repaired copy: it removes faces with zero-length edges and flips
faces pointing the wrong way, both on by default, and can recompute face
normals with `--fix-face-normals true`.

## Profiles

The user library holds the profiles you have installed. `profiles list` shows
them, optionally narrowed to `printers` or `materials`:

```
sc-slicer-cli profiles list printers
```

`profiles export` writes selected profiles into a zip archive and
`profiles import` installs one, refusing to replace what is already there
unless `--force` says otherwise:

```
sc-slicer-cli profiles export --printers my-printers.zip
sc-slicer-cli profiles import my-printers.zip
```

## License

AGPL-3.0-only, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
