# sc-sl1cer

A slicing engine for resin 3D printers: takes triangle meshes from
[`sc-mesh-core`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/mesh-core/)
and returns per-layer closed polygons, optionally rasterised to grayscale
pixel layers.

Written for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md).

## Features

- Slice one mesh or a whole scene under a shared layer schedule. Every stack
  from one run has the same layer count and the same Z values, so stacks can
  be merged layer by layer.
- Uniform layer heights with a separate first-layer height.
- Rasterisation to run-length-encoded 8-bit grayscale layers, with 2x, 4x or
  8x anti-aliasing, or none.
- Progress reporting and cooperative cancellation.
- Parallel across layers and meshes, and deterministic: the same input and
  the same build produce byte-identical output.

## Output

A slice is a stack of layers ordered by strictly increasing Z. Each layer
holds polygons, and each polygon is one outer ring plus its holes. Outer
rings wind counter-clockwise and hole rings wind clockwise.

## Usage

```rust
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution,
    SliceProfile,
};
use sc_sl1cer::slice_scene;

let profile = SliceProfile::new(
    BuildVolume::new(218.88, 122.88, 260.0)?,
    PixelResolution::new(11520, 5120)?,
    LayerSchedule::uniform(0.05, 0.05, 400)?,
    Mirror { x: false, y: false },
    AaLevel::Off,
    PackingMode::None,
);

for sliced in slice_scene(&scene, &profile)? {
    for layer in sliced.stack().layers() {
        println!("z = {} mm, {} polygons", layer.z_mm(), layer.polygons().len());
    }
}
```

`slice_scene_with_progress` takes a callback and a `CancelToken` for the same
work; `slice_leaf` slices a single mesh by its node id.

## Requirements

Meshes must be closed manifolds. Slicing reports which ones are not, listing
every invalid mesh in the scene in one pass rather than stopping at the
first.

Only mesh nodes are sliced. CSG and instance nodes return a "not yet
supported" error.

## Conventions

Right-handed, Z up, millimetres, matching `sc-mesh-core`. The build platform
is at Z = 0 and layers are slabs of constant thickness along +Z.

## Not included

File I/O. Mesh formats are in
[`sc-mesh-formats`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/mesh-formats/)
and printer output formats in
[`sc-sla-formats`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/sla-formats/).
No support-structure generation, no rendering, no GPU types.

For the API, see [docs.rs](https://docs.rs/sc-sl1cer).

## License

Apache-2.0, see [LICENSE](./LICENSE.md).
