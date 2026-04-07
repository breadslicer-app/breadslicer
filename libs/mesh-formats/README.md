# sc-mesh-formats

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library to read, write, and
manipulate 3D mesh data. Part of the [Slicer](../../README.md) project.

## Supported formats

| Format               | Read    | Write   | Notes |
|----------------------|:-------:|:-------:|-------|
| STL (`.stl`)         | ✓       | ✓       | Probes the file to pick ASCII or binary. Writes as binary by default. |
| STL ASCII (`.stla`)  | ✓       | ✓       | |
| STL Binary (`.stlb`) | ✓       | ✓       | |
| OBJ                  | ✓       | ✓       | |
| PLY                  | planned | —       | |
| AMF                  | planned | —       | |
| 3MF                  | planned | —       | |

## Public API

### `stl` module

```rust
use sc_mesh_core::scene::Scene;
use sc_mesh_core::IndexedMesh;
use sc_mesh_formats::stl::{
    read_stl,
    read_stla,
    read_stlb,
    write_stla,
    write_stlb
};
```

**Reading**

```rust
// Auto-detect ASCII vs. binary from the stream
let scene: Scene = read_stl(&mut reader)?;

// Explicit ASCII
let scene: Scene = read_stla(&mut reader)?;

// Explicit binary
let scene: Scene = read_stlb(&mut reader)?;
```

All three functions accept any value implementing `Read + Seek` and return a
`Scene` wrapped in `anyhow::Result`.

**Writing**

STL is a single-mesh format. Use `Scene::merge_meshes` to obtain an
`IndexedMesh` instance:

```rust
let scene: Scene = read_stl(&mut reader)?;
let mesh: IndexedMesh = scene.merge_meshes()?.unwrap();

// Write ASCII STL
write_stla(&mesh, &mut writer)?;

// Write binary STL
write_stlb(&mesh, &mut writer)?;
```

Both functions accept any `W: Write`.

## Credits

- The support for STL file format has been heavily inspired on
  [`stl_io`](https://github.com/hmeyer/stl_io).

## License

Apache-2.0 — see [LICENSE](./LICENSE.txt) or the SPDX headers in each source file.
