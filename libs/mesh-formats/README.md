# sc-mesh-formats

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library to read and write
3D mesh files. Part of the
[Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md)
project.

## What this crate is for

`sc-mesh-formats` turns on-disk mesh files into the `sc_mesh_core::Scene` /
`IndexedMesh` types, and writes them back out. It is the file-I/O layer of
the SoulCrafted slicer stack; everything in-memory lives in `sc-mesh-core`.

It is intentionally narrow:

- No GPU or rendering concerns.
- No mesh editing or repair (those live in `sc-mesh-core`).
- No SLA-printer file formats — `.sl1`, `.ctb`, etc. will live in the future
  `sla-formats` crate.

## Supported formats

| Format               | Read    | Write   | Notes |
|----------------------|:-------:|:-------:|-------|
| STL (`.stl`)         | ✓       | ✓       | Probes the stream to pick ASCII or binary on read. Writes as binary by convention. |
| STL ASCII (`.stla`)  | ✓       | ✓       | |
| STL Binary (`.stlb`) | ✓       | ✓       | |
| OBJ                  | ✓       | ✓       | OBJ is multi-mesh; `write_obj` preserves per-object boundaries. |
| PLY                  | planned | —       | |
| AMF                  | planned | —       | |
| 3MF                  | planned | —       | |

## Modules at a glance

- `loader` — `load_mesh_from_path` / `load_mesh_from_reader` dispatch on file
  type and return a `LoadedMesh` (scene + non-fatal warnings).
- `scene_writer` — `write_scene` flattens a `Scene` (baking transforms) and
  writes it out, returning the warnings the flatten step produced.
- `filetypes` — `MeshFileType` (the all-formats enum used on the read side
  and by CLI argument parsing) and `WriteFormat` (the write-side
  invariant — only the formats that can actually be written; conversion
  is `WriteFormat::try_from(MeshFileType)`). `infer_mesh_file_format_from_extension`
  picks a `MeshFileType` from a path.
- `stl` — `read_stl` / `read_stla` / `read_stlb`, `write_stla` /
  `write_stlb`. Readers are auto-detect / ASCII / binary; writers operate
  on a single `IndexedMesh`.
- `obj` — `read_obj` and `write_obj`. The reader produces a `Scene` with
  one `SceneObject` per OBJ object; the writer round-trips that.
- `messages` — `LoadMeshWarning` and friends, the typed warnings readers
  surface (e.g. unrecognised OBJ statements, missing groups).

For the actual API — types, methods, signatures, and examples — see
[docs.rs](https://docs.rs/sc-mesh-formats).

## Reading vs. writing, in one paragraph

Reads happen at the path or stream level: `load_mesh_from_path` infers the
format from the extension (or takes an explicit `MeshFileType`), parses the
file, and returns a `LoadedMesh { scene, warnings }`. Writes go through
`scene_writer::write_scene`, which takes the typed `WriteFormat` so the
unwriteable `MeshFileType` variants (auto-detect-stl, planned formats) are
ruled out at compile time. Multi-object STL exports flatten with
`Scene::merge_meshes` and surface a `MultiObjectMerge` warning so the
caller can tell the user about lost object boundaries.

## Credits

- The STL support is heavily inspired by
  [`stl_io`](https://github.com/hmeyer/stl_io).

## License

Apache-2.0 — see [LICENSE](./LICENSE.txt) or the SPDX headers in each source
file.
