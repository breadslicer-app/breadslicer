# sc-mesh-spatial

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library that caches
spatial queries over [`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/README.md)
types. Part of the
[Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md)
project.

## What this crate is for

`sc-mesh-spatial` keeps the results of geometric queries that callers
ask for repeatedly: object-space AABBs and triangle clusters today,
with room for further structures (BVH, half-edge, slab buckets,
distance fields) as the slicer grows.

It is intentionally narrow:

- No file I/O (that's `sc-mesh-formats`).
- No rendering or GPU types (that's the GUI).
- No slicing or supports.

The dependency direction is one-way: `sc-mesh-spatial` reads from
`sc-mesh-core` and never the other way around.

## Conventions

Same as `sc-mesh-core`: right-handed, Z-up, millimetres. See the
[core crate's README](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/README.md)
for the full convention list.

## Modules at a glance

- `SceneSpatial`: owns a `Scene` and caches per-object spatial data.
  Two constructors, `new` (lazy) and `primed` (eager); both produce a
  value that is `Send` and safe to build on a worker thread.
- `ObjectQuery`: borrowed view over a single scene object that exposes
  cached queries (`local_aabb`, `clusters`) alongside the underlying
  `SceneObject` and `IndexedMesh`.
- `TriangleClusters`: spatially-grouped triangle ranges with per-cluster
  AABBs, useful as a building block for picker and ray-traversal code.
- `scene_aabb`: world-space AABB of a whole scene, computed from cached
  per-object AABBs.

For the actual API (types, methods, signatures, and example-based
docs) see [docs.rs](https://docs.rs/sc-mesh-spatial).

## Mutation

Scenes mutate through `SceneSpatial::apply_edit`, which takes a
`SceneEdit` from `sc-mesh-core` and invalidates exactly the cached
queries that the edit affects. There is no direct mutation entry
point; querying and mutating both go through `SceneSpatial`.

## License

Apache-2.0, see [LICENSE](./LICENSE.txt) or the SPDX headers in each
source file.
