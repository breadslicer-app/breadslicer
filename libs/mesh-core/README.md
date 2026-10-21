# sc-mesh-core

Triangle meshes for Rust: indexed mesh storage, affine transforms,
bounding boxes, mesh validation, and mesh repair.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md),
a slicer for resin 3D printers, but it depends on nothing else in that
project and stands on its own.

## Features

- Indexed triangle meshes with optional metadata.
- Affine transforms and axis-aligned bounding boxes.
- Validation for non-finite coordinates, out-of-range face indices,
  zero-length edges, holes, edges shared by more than 2 faces and
  inconsistent winding, with repairs for some of those.

The hierarchical scene graph lives in
[`sc-scene`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/scene/),
reading and writing mesh files (STL, OBJ, 3MF) lives in
[`sc-mesh-formats`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-formats/),
and slicing lives in
[`sc-sl1cer`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/sl1cer/).
This crate carries no rendering or GPU types.

## Conventions

- **Right-handed, Z up.** The XY plane is the build platform.
- **Millimetres** for every coordinate, length and translation. There is no
  unit type, and readers of unit-less formats treat the file's numbers as
  millimetres.
- Vertex positions are `f32`; transform matrices are `f64`.

## Validation

Two things about the validation API are worth knowing before you call it.

Each check returns every problem it found rather than the first one, so
fixing a broken mesh is one pass instead of a game of whack-a-mole.

Checks that presuppose valid face indices are reachable only through a
witness type minted by the check that establishes them. A function needing a
closed manifold takes proof that it has one, so it never re-validates
defensively and you never pay for the same check twice.

For the API, see [docs.rs](https://docs.rs/sc-mesh-core).

## License

Apache-2.0, see [LICENSE](./LICENSE.md).
