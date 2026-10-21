# sc-scene

A hierarchical scene graph over
[`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/)
meshes: groups, mesh-backed leaves, shared mesh resources, subtree
references, CSG placeholders, plus expansion, world-space baking,
validation and repair orchestration.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md),
a slicer for resin 3D printers.

## Features

- Arena-backed scene graph with stable, never-reused identifiers.
- Groups, inline-mesh and mesh-resource leaves, registered subtrees
  with reference nodes, and CSG nodes carrying an operator.
- Occurrence expansion (`for_each_expanded_leaf`) with an explicit CSG
  policy, and world-space baking of leaves or whole scenes.
- Scene-level AABB, structural validation (dangling ids, cycles,
  orphans), mesh-health sweeps, and repair orchestration that forks
  shared meshes only when a repair would change them.

Typed undo / redo edits and clipboard fragments live in
[`sc-scene-edit`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/scene-edit/);
file I/O lives in
[`sc-mesh-formats`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-formats/);
slicing lives in
[`sc-sl1cer`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/sl1cer/).
This crate carries no rendering or GPU types.

## Conventions

Same as `sc-mesh-core`: right-handed, Z up, millimetres.

For the API, see [docs.rs](https://docs.rs/sc-scene).

## License

Apache-2.0, see [LICENSE](./LICENSE.md).
