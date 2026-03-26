# sc-mesh-core

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library with core types for
handling 3D mesh data. Part of the
[Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md)
project.

## Overview

This crate defines fundamental data structures used across the SoulCrafted
slicer libraries. Other crates (e.g. `sc-mesh-formats`) depend on these types
to represent and exchange mesh data.

## Public API

### `IndexedMesh`

**Fields:**
- `meta: Option<MeshMetadata>`: optional metadata.
- `vertices: Vec<Vertex>`: the vertex buffer.
- `faces: Vec<IndexedTriangle>`: triangle faces with indexed vertices and a
  normal vector each.

**Validation methods:**

All validators return `Ok(())` on success, or a `Vec` of specific error enums
describing every issue found:

- `validate_vertices()`: detects `NaN` and `Infinity` coordinates.
- `validate_faces()`: detects out-of-range vertex indices and zero-length edges.
- `validate_normal_vectors()`: detects zero-area triangles, non-unit normals,
  and normals that don't match the vertex winding order.
- `validate_manifold()`: detects boundary edges, non-manifold edges,
  inconsistent winding order, and empty meshes.

**Fix methods:**

- `fix_faces_orientation(&mut self, manifold_errors, normal_vector_errors)`:
  flips winding order for incorrectly oriented faces and recomputes their
  normals. Accepts error slices from `validate_manifold` and
  `validate_normal_vectors`.
- `fix_zero_length_edges(&mut self, face_errors)`: removes faces that have
  collapsed (zero-length) edges. Accepts the error slice from `validate_faces`.

Both return a `Vec<MeshFix>` listing every repair applied.

## License

Apache-2.0 — see [LICENSE](./LICENSE.txt) or the SPDX headers in each source file.
