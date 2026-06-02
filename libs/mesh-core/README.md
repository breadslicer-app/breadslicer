# sc-mesh-core

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library with core types for
3D meshes and scenes. Part of the
[Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md)
project.

## What this crate is for

`sc-mesh-core` is the shared vocabulary of the SoulCrafted slicer stack: the
data structures that the format readers (`sc-mesh-formats`), the future
slicer engine (`sl1cer`), the support generator, and the GUI all speak.
Anything that needs to describe a 3D mesh, group meshes into a scene, place
them in the world, or check that they're well-formed enough to print lives
here.

It is intentionally narrow:

- No file I/O (that's `sc-mesh-formats`).
- No rendering, no GPU types (that's the GUI).
- No slicing or supports.

## Conventions

- **Coordinate frame:** right-handed, **Z up**. The XY plane is the build
  platform; +Z points away from it.
- **Units:** every coordinate, length, and translation is in **millimetres**.
  Vertex positions are stored as `f32`; transform matrices are `f64`. Both
  share the same millimetre scale; there is no unit type yet.

## Modules at a glance

- `primitives`: `Vertex`, `Normal`, `Vector3D`, `Triangle`, `IndexedTriangle`.
- `indexed_mesh`: `IndexedMesh` (vertex buffer + indexed face buffer + optional
  metadata), plus mesh validation and repair routines. The witness chain
  (`ValidatedFaces` for in-range face indices, `ValidatedManifoldMesh` for
  closed manifolds) lets downstream APIs require the invariant they need
  at the type level instead of re-validating defensively.
- `aabb`: `Aabb` axis-aligned bounding boxes.
- `transform`: `AffineTransform3D` 3×4 affine transforms in `f64`, with
  composition, inversion, and a normal-aware apply.
- `scene`: `Scene`, `SceneObject`, `Transform3D` (`Identity` /
  `Affine`-tagged) for placing meshes in world space, plus `Scene::aabb` and
  `Scene::merge_meshes` (transform baking).
- `scene_edit`: `SceneEdit`, the typed mutation vocabulary over `Scene`
  (variants + `apply_to_scene`). Cache layers like `sc-mesh-spatial`
  dispatch on the same enum to attach invalidation hints.
- `errors`: typed error enums returned by validators and transform routines,
  including `MeshVertexError`, `MeshFaceError`, `MeshNormalVectorError`,
  `MeshManifoldError`, `MeshFix`, `TransformError`, plus `Edge`,
  `VertexIndex`, `FaceIndex` and friends.
- `geometry`: small numeric helpers used across the crate (singularity
  threshold, strict comparisons).

For the actual API (types, methods, signatures, and example-based docs)
see [docs.rs](https://docs.rs/sc-mesh-core).

## Validation

Mesh validation is split by concern: vertices, face indices, normal vectors,
and manifoldness each have a dedicated `validate_*` method on `IndexedMesh`,
and each returns `Result<(), Vec<…Error>>` listing every issue it found
rather than bailing on the first. Repair routines (`fix_faces_orientation`,
`fix_zero_length_edges`) consume those error vectors and return a
`Vec<MeshFix>` describing what was actually changed.

## License

Apache-2.0, see [LICENSE](./LICENSE.txt) or the SPDX headers in each source
file.
