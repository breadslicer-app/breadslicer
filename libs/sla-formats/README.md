# sc-sla-formats

Writes the per-layer geometry produced by
[`sc-sl1cer`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/sl1cer/)
out to disk, either in a format a resin printer accepts or in one meant to be
looked at.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md).

This crate only writes. Reading printer files back is not a goal, and mesh
formats belong to
[`sc-mesh-formats`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-formats/).

## Formats

| Name            | Kind    | Output    | Notes |
|-----------------|---------|-----------|-------|
| `svg-per-layer` | debug   | directory | One SVG per layer. |
| `svg-combined`  | debug   | file      | Every layer as a group in one SVG. |
| `png-per-layer` | debug   | directory | One 8-bit grayscale PNG per layer, rasterised with the run's anti-aliasing. |
| `3mf`           | printer | file      | A package using the [3MF Slice Extension](https://github.com/3MFConsortium/spec_slice), conventionally named `*.slices.3mf`. |
| SL1, CTB, GOO   | printer | -         | Planned. Asking for one today returns `WriterError::NotImplemented`. |

Debug formats are for inspection, regression fixtures and visual checking,
not for feeding to a printer. Each layer file is self-describing: the SVG
writers embed a metadata block and the PNG writer uses text chunks, both
carrying the slicer version, build volume, pixel resolution, anti-aliasing
level and the layer's Z.

## Usage

Every writer takes a `WriterOptions`, built from the same `SliceProfile` the
slicing run used, so the output geometry matches what was sliced.

### Writing debug layers

`write_slice` takes one contour stack. The output path is a directory for
per-layer formats and a file for packed ones, and it is checked against the
format before anything is written: a directory is created if absent and
refused if it already holds entries.

```rust
use std::path::Path;

use sc_sla_formats::{Format, WriterOptions, write_slice};

let options = WriterOptions::from_profile(&profile, sc_sl1cer::VERSION);

write_slice(&stack, Path::new("layers"), Format::PngPerLayer, &options)?;
write_slice(&stack, Path::new("all-layers.svg"), Format::SvgCombined, &options)?;
```

Raster formats can hold a minimum exposure for partially covered pixels:

```rust
let options = WriterOptions::from_profile(&profile, sc_sl1cer::VERSION)
    .with_min_alpha_pct(20.0);
```

### Writing a 3MF slice package

This one describes the whole plate, so it takes the scene and the per-object
stacks rather than a single merged stack, and has its own entry point. Asking
`write_slice` for it returns `WriterError::SceneContextRequired`.

```rust
use std::path::Path;

use sc_sl1cer::slice_scene;
use sc_sla_formats::{WriterOptions, write_scene_slices};

let sliced = slice_scene(&scene, &profile)?;
let options = WriterOptions::from_profile(&profile, sc_sl1cer::VERSION);

write_scene_slices(&scene, &sliced, Path::new("plate.slices.3mf"), &options)?;
```

Each sliced object becomes one object in the package, carrying its mesh baked
to world space plus its slice stack, with the layer data in a separate part.
Meshes are embedded at full resolution, so a program with no slice support
still opens the file and sees the geometry.

The layer heights recorded in the package are the slab tops derived from the
layer schedule in `WriterOptions`, not the contour layers' own Z, which are
the sampling planes in the middle of each slab. A stack whose layer count
does not match that schedule is refused with
`WriterError::LayerGridMismatch`.

For the API, see [docs.rs](https://docs.rs/sc-sla-formats).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
