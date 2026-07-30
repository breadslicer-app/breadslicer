# sc-project-formats

Reads and writes slicer project files: a
[`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/)
scene together with the printer and resin it is meant for, optional copies of
those profile documents, and optionally the slice result.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md).

A project file is an ordinary
[3MF](https://github.com/3MFConsortium/spec_core) package with extra parts
alongside the model, so any other tool still opens it and sees the geometry.
Those parts are linked with the Open Packaging Conventions MustPreserve
relationship, so an editor that rewrites the package keeps them.

## Features

- Read and write projects, with progress callbacks for the compression,
  which dominates on a large file.
- Carry which printer and resin the project uses, plus the chosen first
  layer height.
- Embed copies of the profile documents so the file works on a machine that
  does not have them installed, and find out which references resolve
  nowhere.
- Store the computed slices so reopening a project does not mean re-slicing.
- Open a plain 3MF that has no project data, and detect project data written
  by PrusaSlicer, Bambu Studio, Orca or Cura.

## Usage

### Reading

The scene always comes back. `payload` is `None` for a file with no project
data of ours, such as a plain 3MF, and `warnings` reports anything the file
carried that was not understood.

```rust
use std::fs::File;

use sc_project_formats::{ProjectFileType, read_project};

let mut file = File::open("bust.3mf")?;
let project = read_project(&mut file, ProjectFileType::ThreeMf)?;

println!("{} root nodes", project.scene.roots().len());

if let Some(payload) = &project.payload {
    println!("printer: {:?}, resin: {:?}", payload.printer, payload.resin);
}

for warning in &project.warnings {
    eprintln!("{warning:?}");
}
```

### Checking that the referenced profiles are available

A project names its printer and resin by id. Supply the two predicates that
answer whether an id resolves on this machine, and each reference that does
not comes back either as missing outright or as present only as an embedded
copy inside the file.

```rust
if let Some(payload) = &project.payload {
    for warning in payload.profile_ref_warnings(
        |id| installed_printers.contains(id),
        |id| installed_resins.contains(id),
    ) {
        eprintln!("{warning:?}");
    }
}
```

### Writing

Pass `EmbedProfiles::Yes` to write the payload's embedded documents into the
file, or `EmbedProfiles::No` to write only the references.

```rust
use std::fs::File;

use sc_project_formats::{EmbedProfiles, EmbeddedProfile, ProjectFileType, ProjectPayload, write_project};

let payload = ProjectPayload {
    printer: Some("elegoo-saturn-4-ultra-ctb".parse()?),
    resin: Some("sirayatech-blu-saturn4".parse()?),
    first_layer_height: None,
    embedded: vec![EmbeddedProfile::Printer {
        id: "elegoo-saturn-4-ultra-ctb".parse()?,
        bytes: std::fs::read("elegoo-saturn-4-ultra.toml")?,
    }],
};

let mut out = File::create("bust.3mf")?;
write_project(
    &scene,
    &payload,
    EmbedProfiles::Yes,
    None, // slices
    None, // slice validity token
    ProjectFileType::ThreeMf,
    &mut out,
)?;
```

### Storing slices with the project

Pass a `PlateSlices`, the merged contour stack for the whole plate plus the
top Z of each layer, and a token of your own that identifies what the slices
were computed from. Reading gives both back untouched; deciding whether the
token still matches the scene is yours, since only you know what went into
it.

```rust
use sc_project_formats::PlateSlices;

let slices = PlateSlices { contours, slab_tops };

write_project(
    &scene,
    &payload,
    EmbedProfiles::Yes,
    Some(&slices),
    Some("sha256:..."),
    ProjectFileType::ThreeMf,
    &mut out,
)?;
```

### Reporting progress

Both sides have a `_with_progress` variant. Writing distinguishes
serialisation from compression; reading ticks during container extraction
only, not during parsing.

```rust
use sc_project_formats::{WriteProgress, write_project_with_progress};

write_project_with_progress(
    &scene, &payload, EmbedProfiles::No, None, None,
    ProjectFileType::ThreeMf, &mut out,
    |progress| match progress {
        WriteProgress::Serialize { done, total } => println!("xml {done}/{total}"),
        WriteProgress::Compress { done, total } => println!("zip {done}/{total}"),
    },
)?;
```

## Reading files this build does not fully understand

- A 3MF without our project part loads as a plain model, with no payload.
- A payload version this build does not know loads its scene and warns that
  the project data was ignored; re-saving will not carry it.
- Project data from another slicer is recognised by its marker parts and
  reported, never parsed, and re-saving will not carry it.
- Embedded profile documents are held as raw bytes and never parsed here, so
  a re-save writes back exactly what was read.

For the API, see [docs.rs](https://docs.rs/sc-project-formats).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
