# sc-scene-edit

Typed mutation vocabulary for
[`sc-scene`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/scene/)
graphs: invertible edits for undo / redo stacks, and self-contained
scene fragments for clipboard-style transfer between scenes.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md),
a slicer for resin 3D printers.

## Features

- `SceneEdit`: one enum variant per supported scene mutation, keyed by
  stable identifiers, each with an exact inverse; compounds group
  several edits into a single undo / redo unit.
- `SceneFragment`: a freestanding scene wrapping the copied nodes and
  mesh resources, preserving `Arc` identity so a paste can deduplicate
  shared meshes in the target scene.

## Conventions

Same as `sc-mesh-core`: right-handed, Z up, millimetres.

For the API, see [docs.rs](https://docs.rs/sc-scene-edit).

## License

Apache-2.0, see [LICENSE](./LICENSE.md).
