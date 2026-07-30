# sc-3mf

A Rust implementation of [3MF](https://3mf.io/specification/), the 3D
Manufacturing Format published by the 3MF Consortium: the package container
and a typed model document.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md),
a slicer for resin 3D printers, but it carries no application types and can be
used on its own.

## Features

- Read and write the package container, an
  [Open Packaging Conventions](https://www.ecma-international.org/publications-and-standards/standards/ecma-376/)
  (ECMA-376 Part 2) ZIP archive: named parts, content types declared by
  extension or per part, and relationship parts. Relative relationship
  targets resolve against the source part's directory.
- Read and write the 3D model part defined by the
  [3MF Core Specification](https://github.com/3MFConsortium/spec_core):
  objects with mesh or component payloads, build items with transforms, base
  material groups, metadata, and the resource id space.
- Read and write the
  [3MF Slice Extension](https://github.com/3MFConsortium/spec_slice), which
  stores a model as stacked 2D cross-sections: slice stacks attached to
  objects, inline slices with their polygons, slice references into other
  model parts, and the `meshresolution` marker that says whether an object's
  mesh is authoritative or a stand-in for the slice data.
- Progress callbacks on both reading and writing, which on a large package is
  mostly time spent compressing.
- Parts above a size threshold are compressed across threads and spliced into
  the archive as ordinary ZIP entries.

Coordinates stay in the unit the document declares. The crate gives you the
conversion factor to millimetres; applying it is yours to do.

## Usage

### Reading

Open the archive, follow the package's StartPart relationship to the 3D model
part, and parse that part into a document.

```rust
use std::fs::File;

use sc_3mf::model::parse_model;
use sc_3mf::package::Package;

let package = Package::read_from(File::open("model.3mf")?)?;

let start = package.start_part()?;
let bytes = package.part_data(&start).expect("start part is present");
let model = parse_model(bytes)?;

// Document lengths are in `model.unit`; convert them yourself.
let to_mm = model.unit.scale_to_mm();
println!("{} objects, {} build items", model.objects.len(), model.build.len());
```

### Writing

Build a document, serialise it to XML, add it to a package as the 3D model
part, point the StartPart relationship at it, and write the archive.

```rust
use std::fs::File;

use sc_3mf::model::{
    BuildItem, MeshData, ModelDocument, ObjectPayload, ObjectResource, ObjectType, ResourceId,
    ResourceIndex, TriangleRef, Unit, Vertex3, write_model,
};
use sc_3mf::names::{DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::{Package, PartName, Relationship};

let corner = |i| ResourceIndex::new(i).expect("index within range");
let object_id = ResourceId::MIN;

let model = ModelDocument {
    unit: Unit::Millimeter,
    objects: vec![ObjectResource {
        id: object_id,
        object_type: ObjectType::Model,
        name: Some("tetrahedron".into()),
        partnumber: None,
        property_ref: None,
        slice_stack: None,
        mesh_resolution: Default::default(),
        payload: ObjectPayload::Mesh(MeshData {
            vertices: vec![
                Vertex3 { x: 0.0, y: 0.0, z: 0.0 },
                Vertex3 { x: 10.0, y: 0.0, z: 0.0 },
                Vertex3 { x: 0.0, y: 10.0, z: 0.0 },
                Vertex3 { x: 0.0, y: 0.0, z: 10.0 },
            ],
            triangles: vec![
                TriangleRef { v1: corner(0), v2: corner(2), v3: corner(1) },
                TriangleRef { v1: corner(0), v2: corner(1), v3: corner(3) },
                TriangleRef { v1: corner(1), v2: corner(2), v3: corner(3) },
                TriangleRef { v1: corner(0), v2: corner(3), v3: corner(2) },
            ],
        }),
    }],
    build: vec![BuildItem { object_id, transform: None, partnumber: None }],
    ..Default::default()
};

let mut xml = Vec::new();
write_model(&model, &mut xml)?;

let part = PartName::new(DEFAULT_MODEL_PART_NAME)?;
let mut package = Package::new();
package.add_part(part.clone(), MODEL_PART_CONTENT_TYPE, xml)?;
package.add_root_relationship(Relationship {
    id: "rel0".into(),
    rel_type: START_PART_REL_TYPE.into(),
    target: part.as_str().into(),
});

package.write_to(File::create("tetrahedron.3mf")?)?;
```

## Limits

Reading caps the decompressed size of an archive at 2 GiB for any single part
and 4 GiB for the package.

## Not supported

Interleaved packages and ZIP64 writing (ZIP64 reading comes from the
underlying `zip` crate). The
[3MF Production Extension](https://github.com/3MFConsortium/spec_production),
which splits a model across several parts, is detected but not implemented,
so such a document is refused rather than partially read.

## Testing

The test suite parses unmodified sample packages from the 3MF Consortium's
[sample repository](https://github.com/3MFConsortium/3mf-samples). See
`tests/fixtures/official/README.md` for their provenance and license.

For the API, see [docs.rs](https://docs.rs/sc-3mf).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
