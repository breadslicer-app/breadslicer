# sc-mesh-core

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library with core types for
handling 3D mesh data. Part of the [Slicer](../../README.md) project.

## Overview

This crate defines fundamental data structures used across the SoulCrafted
slicer libraries. Other crates (e.g. `sc-mesh-formats`) depend on these types
to represent and exchange mesh data.

## Public API

### Types

```rust
use sc_mesh_core::{
    Vector3D,
    Vertex,        // alias for Vector3D<f32>
    Normal,        // alias for Vector3D<f32>
    Triangle,
    IndexedTriangle,
    IndexedMesh,
    MeshMetadata,
};
```

## License

Apache-2.0 — see [LICENSE](./LICENSE.txt) or the SPDX headers in each source file.
