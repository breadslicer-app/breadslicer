# sc-mesh-formats

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library to read and write
3D mesh files. Part of the
[Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md)
project.

## What this crate is for

`sc-mesh-formats` turns on-disk mesh files into the scene and mesh types of
[`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/),
and writes them back out. It is the file-I/O layer of the SoulCrafted slicer
stack; everything in-memory lives in `sc-mesh-core`.

It is intentionally narrow:

- No GPU or rendering concerns.
- No mesh editing or repair (those live in `sc-mesh-core`).
- No SLA-printer file formats; `.sl1`, `.ctb` and friends live in
  [`sc-sla-formats`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/sla-formats/).
- No project files; those live in
  [`sc-project-formats`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/project-formats/).

## Supported formats

| Format               | Read    | Write   | Notes |
|----------------------|:-------:|:-------:|-------|
| STL (`.stl`)         | yes     | yes     | Probes the stream to pick ASCII or binary on read. Writes as binary by convention. |
| STL ASCII (`.stla`)  | yes     | yes     | |
| STL Binary (`.stlb`) | yes     | yes     | |
| OBJ (`.obj`)         | yes     | yes     | Multi-mesh; per-object boundaries survive the round trip. |
| 3MF (`.3mf`)         | yes     | yes     | Core specification. Shared meshes and group structure survive the round trip instead of being flattened. |
| PLY (`.ply`)         | planned | planned | |
| AMF (`.amf`)         | planned | planned | |

The read side and the write side have separate types.

## Flattening

Formats differ in what structure they can hold. OBJ and 3MF keep the scene's
structure. STL cannot, so a scene is flattened before it is written: a
single-mesh scene keeps its metadata, and a multi-mesh scene is merged and
reports a warning, so the caller can tell the user that object boundaries were
lost rather than letting them discover it later.

## Limits

Readers refuse pathological input instead of exhausting memory on it. A
path-level load checks the file's on-disk size against a ceiling before
opening it, and every reader caps how many elements of a single kind it will
allocate for, whether that is a binary STL's declared triangle count or an
OBJ's vertex count.

For the API, see [docs.rs](https://docs.rs/sc-mesh-formats/latest/sc_mesh_formats/).

## Credits

- The STL support is heavily inspired by
  [`stl_io`](https://github.com/hmeyer/stl_io).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
