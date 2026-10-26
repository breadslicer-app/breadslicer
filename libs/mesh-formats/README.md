# sc-mesh-formats

Reads and writes 3D mesh files as
[`sc-mesh-core`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/mesh-core/)
scenes and meshes.

Written for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md),
a slicer for resin 3D printers.

## Supported formats

| Format               | Read    | Write   | Notes |
|----------------------|:-------:|:-------:|-------|
| STL (`.stl`)         | yes     | yes     | Detects ASCII or binary on read. Writes as binary by convention. |
| STL ASCII (`.stla`)  | yes     | yes     | |
| STL Binary (`.stlb`) | yes     | yes     | |
| OBJ (`.obj`)         | yes     | yes     | Multi-mesh; per-object boundaries survive the round trip. |
| 3MF (`.3mf`)         | yes     | yes     | Core specification. Shared meshes and group structure survive the round trip. |
| PLY (`.ply`)         | planned | planned | |
| AMF (`.amf`)         | planned | planned | |

## Usage

```rust
use std::path::Path;

use sc_mesh_formats::filetypes::WriteFormat;
use sc_mesh_formats::load_mesh_from_path;
use sc_mesh_formats::scene_writer::write_scene;

// Format inferred from the extension; pass `Some(kind)` to override it.
// The third argument caps the accepted file size in bytes.
let loaded = load_mesh_from_path(Path::new("model.obj"), None, None)?;

for warning in &loaded.warnings {
    eprintln!("{warning:?}");
}

let flatten_warnings =
    write_scene(&loaded.graph, Path::new("model.stl"), WriteFormat::StlBin)?;
```

Reading and writing use separate format types: the read side names every
format the loader accepts, the write side only those a writer exists for, and
the conversion between them is fallible.

## Flattening

- OBJ and 3MF keep the scene's structure.
- STL cannot hold one, so a scene is flattened before it is written. A
  single-mesh scene keeps its metadata; a multi-mesh scene is merged, and the
  write returns a warning carrying the number of objects that were merged.

## Limits

A load from a path checks the file's size against the cap you pass before
opening it. Every reader also caps how many elements of a single kind it will
allocate for, whether that is a binary STL's declared triangle count or an
OBJ's vertex count.

## Not included

Mesh editing and repair, which are in
[`sc-mesh-core`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/mesh-core/);
printer output formats such as `.sl1` and `.ctb`, which are in
[`sc-sla-formats`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/sla-formats/);
project files, which are in
[`sc-project-formats`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/project-formats/).
No rendering or GPU types.

For the API, see [docs.rs](https://docs.rs/sc-mesh-formats/).

## Credits

- The STL support is heavily inspired by
  [`stl_io`](https://github.com/hmeyer/stl_io).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
