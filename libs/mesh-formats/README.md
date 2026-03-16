# sc-mesh-formats

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library to read, write, and
manipulate 3D mesh data. Part of the [Slicer](../../README.md) project.

## Supported formats

| Format               | Read    | Write | Notes |
|----------------------|:-------:|:-----:|-------|
| STL (`.stl`)         | ✓       | —     | Probes the file to pick ASCII or binary |
| STL ASCII (`.stla`)  | ✓       | ✓     | |
| STL Binary (`.stlb`) | ✓       | ✓     | |
| OBJ                  | planned | —     | |
| PLY                  | planned | —     | |
| AMF                  | planned | —     | |
| 3MF                  | planned | —     | |

## Public API

### `stl` module

```rust
use sc_mesh_formats::core::IndexedMesh; // Just for type annotation
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
let mesh: IndexedMesh = read_stl(&mut reader)?;

// Explicit ASCII
let mesh: IndexedMesh = read_stla(&mut reader)?;

// Explicit binary
let mesh: IndexedMesh = read_stlb(&mut reader)?;
```

All three functions accept any value implementing `Read + Seek` and return an
`IndexedMesh` wrapped in `anyhow::Result`.

**Writing**

```rust
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
