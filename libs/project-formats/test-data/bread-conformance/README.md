# Conformance files

Small `.bread` files for testing a reader against [SPEC.md](../SPEC.md).

## Valid files

Every file in [`valid/`](valid) is a valid `.bread` file, and a conforming reader MUST open it.
Beside each `name.bread` is a `name.json` that says what the file holds. A reader passes a file when
what it reads matches the JSON.

| Field | Meaning |
|---|---|
| `description` | what the file shows, in a sentence |
| `meta` | the scene's metadata: `null`, or an object with `name` and `comment`, each a string or `null` |
| `meshes` | one object per `MESH` chunk, in file order (below) |
| `mesh_resources` | the mesh resources list: a mesh index each |
| `nodes` | one object per node record, in order (below) |
| `subtrees` | the root node index of each subtree record, in order |
| `roots` | the scene's root node indices, in order |
| `settings` | the `PROJ` payload: `null` when there is none, or its `size` in bytes and its `sha256` |
| `supports` | the `SUPP` entries, in order (below) |
| `hollows` | the `HOLW` entries, in order (below) |
| `thumbnail` | the `THMB` payload, a PNG: `null` when there is none, or its `size` in bytes and its `sha256` |
| `support_previews` | the `SPRV` previews, in order: each has `leaf`, the support leaf's node index, and `mesh`, as for the meshes |
| `surface` | the `SURF` entries, in order: each has `leaf`, `kind`, the document `version`, and the document's `document_size` and `document_sha256` |

Each mesh:

| Field | Meaning |
|---|---|
| `meta` | the mesh's metadata, as for the scene |
| `vertex_count`, `triangle_count` | how many vertices and triangles the mesh has |
| `vertices_sha256` | the SHA-256 of the vertices: x, y and z of each vertex in order, each as the 4 bytes of its `f32`, little-endian |
| `triangles_sha256` | the SHA-256 of the triangles: the three vertex indices of each triangle in order, first corner first, each as a `u32`, little-endian |

Each node has a `kind`, its `meta` as for the scene, and its `transform`: `null` for a node stored
without one (the identity), or an object with `linear`, the 3 × 3 matrix as three rows, and
`translation`. Each number is the shortest decimal that reads back as the same `f64`. Then, by kind:

| `kind` | Field | Meaning |
|---|---|---|
| `"group"` | `children` | the child node indices, in order |
| `"leaf"` | `mesh` | the mesh index |
| `"shared leaf"` | `resource` | the mesh resource index |
| `"reference"` | `subtree` | the subtree index |

Each supports entry has `model` and `support`, the two leaves' node indices; each hollows entry has
`model` and `original`, the original's mesh index. Both then have the `generator` name, its document
`version`, and the document's `document_size` in bytes and its `document_sha256`.

## Invalid files

Every file in [`invalid/`](invalid) is damaged or breaks a rule of the format, and a conforming
reader MUST refuse it. Each is a valid file with one thing changed, most of them
[`valid/triangle.bread`](valid/triangle.bread). Beside each `name.bread` is a `name.json`:

| Field | Meaning |
|---|---|
| `description` | what is wrong with the file, in a sentence |
| `reason` | why a reader refuses it: one of the codes below |
| `chunk` | the tag of the chunk the reason is about, or `null` when it is about the whole file |

A reader passes a file when it refuses it. A reader that tells the reasons apart can also check
that it gives the expected one:

| `reason` | The reader finds | Section |
|---|---|---|
| `not-bread` | fewer than 8 bytes, or 8 bytes that are not the magic | [4.4](../SPEC.md#44-reading-a-file) |
| `version` | the container, or a chunk it reads, at a version newer than the ones it supports, or at version 0 | [3.4](../SPEC.md#34-versions) |
| `cut-short` | the file ends inside the container version, a chunk header or the stored bytes | [4.4](../SPEC.md#44-reading-a-file) |
| `crc` | stored bytes that do not match their CRC | [4.3](../SPEC.md#43-stored-bytes-and-checksum) |
| `bad-payload` | stored bytes that are not exactly one valid frame, or that do not decompress to exactly the raw length | [4.4](../SPEC.md#44-reading-a-file) |
| `chunk-count` | more or fewer chunks of a tag than the format allows, such as two scene chunks | [7.1](../SPEC.md#71-official-chunks) |
| `malformed` | a payload that is malformed: cut short, padded, a value the format does not allow, a count larger than the bytes left can hold, or a chunk that names something that does not exist or breaks a rule about what it names | [3.3](../SPEC.md#33-malformed-payloads), [5.3](../SPEC.md#53-what-a-reader-checks), [6](../SPEC.md#6-mesh-mesh), [7.3](../SPEC.md#73-application-chunks), [9.2](../SPEC.md#92-reading-it), [10.2](../SPEC.md#102-reading-it) |
| `no-such-vertex` | a triangle that names a vertex its mesh does not have | [6.4](../SPEC.md#64-triangles) |

Some files hold a length or a count far larger than what backs it. A reader that allocates for it
before checking it, as [4.5](../SPEC.md#45-size-limits) forbids, runs out of memory on them
instead of refusing them.

## Making the files again

The files are made by the cases in
[`crates/bread-format/tests/conformance.rs`](../crates/bread-format/tests/conformance.rs). Its tests
check that the crate still writes the same bytes for each case, reads each valid file as its JSON
says, and refuses each invalid file for its reason. After a change on purpose, the files are made
again with:

```text
cargo test -p bread-format --test conformance -- --ignored
```
