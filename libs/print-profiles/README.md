# sc-print-profiles

Printer and resin profiles for resin 3D printing: the TOML file format, a
library of them on disk, and the resolution that turns a chosen printer, a
chosen resin, and any per-print adjustments into one set of values to print
with.

Written for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md).

## Features

- Load a printer or resin profile from a file, or by id from the user's
  library with a fallback to the profiles an installation ships.
- Resolve a printer and a resin into one configuration: layer height,
  exposure times, bottom layer count, lift distance and speeds, and scale
  compensation.
- Get back, alongside the values, a record of which fell back to the resin's
  defaults, which were derived from its intent, which a printer constraint
  moved, and which an override replaced.
- Save a printer or resin profile into the library, together with a
  printer's preview image, and delete resin profiles from it.
- List the profiles in a library, and move them between machines as zip
  archives.

## Usage

### Resolving a print

Load a printer and a resin from files, then combine them into the values to
print with. `SceneOverrides::default()` means "no per-print adjustments".

```rust
use sc_print_profiles::{
    EffectiveMaterial, SceneOverrides, load_material_profile_from_path,
    load_printer_profile_from_path, resolve,
};

let printer = load_printer_profile_from_path("elegoo-saturn-4-ultra.toml".as_ref())?;
let resin = load_material_profile_from_path("sirayatech-blu.toml".as_ref())?;
let material = EffectiveMaterial::from(resin);

let config = resolve(&printer, &material, &SceneOverrides::default())?;

let values = &config.material_resolved;
println!(
    "{} mm layers, {} s exposure, {} bottom layers",
    values.layer_height_mm,
    values.normal_exposure_sec,
    values.bottom_layer_count,
);

// Why each value ended up as it did: fell back to the resin's defaults,
// was derived from its intent, or was moved by a printer constraint.
for warning in &config.warnings {
    eprintln!("{warning:?}");
}
```

### Adjusting values for one print

Populate the fields of `SceneOverrides` you want to change. Overrides are
still subject to the printer's constraints.

`overrides_applied` is the trail of how every value was decided, not only the
overridden ones: one record per field, in decision order, naming the source
(`Calibrated`, `Derived`, `Default` or `Override`) along with the prior and
final values. An overridden field appears twice, once for the value it
started with and once for the one that replaced it.

```rust
use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{SceneOverrides, resolve};

let overrides = SceneOverrides {
    layer_height_mm: Some(PositiveFiniteMm::new(0.03)?),
    ..SceneOverrides::default()
};

let config = resolve(&printer, &material, &overrides)?;

for record in &config.overrides_applied {
    println!("{record:?}");
}
```

### Using the installed libraries

Profiles the user has installed are addressed by id rather than by path.
`ProfileLibraries` names the libraries to search, highest precedence
first: the user's writable library, then the read-only libraries the
installation ships. Loading by id returns the profile from the first
library that has it; when none has it, the error lists every path that
was tried. A system without a resolvable user library searches the
bundled libraries alone.

```rust
use sc_print_profiles::{ProfileLibraries, list_printer_ids};

let libraries = ProfileLibraries::platform_default();

if let Some(library) = libraries.user_library_dir() {
    for id in list_printer_ids(library)? {
        println!("{id}");
    }
}

let id = "elegoo-saturn-4-ultra".parse()?;
let printer = libraries.load_printer_by_id(&id)?;
```

### Moving a library between machines

Export writes the selected profiles into a zip archive laid out like the
library itself. Import installs one, and without `force` it refuses rather
than overwrite, listing what would have been replaced.

```rust
use std::fs::File;

use sc_print_profiles::{
    ExportManifest, ExportSelection, ImportError, export_package, import_package,
};

export_package(
    &library,
    &ExportSelection::Printers,
    &ExportManifest {
        name: Some("my printers".into()),
        description: None,
        author: None,
        exported_at: "2026-07-30T12:00:00Z".into(),
        sc_slicer_version: env!("CARGO_PKG_VERSION").into(),
    },
    File::create("my-printers.zip")?,
)?;

match import_package(File::open("my-printers.zip")?, &library, false) {
    Err(ImportError::Collisions(report)) => {
        println!("{} printers already installed", report.printers.len());
    }
    other => other?,
}
```

## Profile files

A **printer profile** describes a machine, in four sections:

| Section         | Holds |
|-----------------|-------|
| `identity`      | Who the printer is. |
| `hardware`      | Build volume, pixel resolution, pixel size, display mirroring. |
| `capabilities`  | Output formats, grayscale support, network protocol. |
| `constraints`   | Layer-height range, Z step, speed ranges, maximum lift distance, light intensity. |

A **resin profile** describes a material, also in four sections:

| Section       | Holds |
|---------------|-------|
| `identity`    | Who the resin is. |
| `intent`      | What it aims for, independent of any printer: a target layer height and a target energy density. |
| `defaults`    | Values that apply on any printer. |
| `per_printer` | Values calibrated for one printer, keyed by its id; the key `generic-machine` holds values that apply to any printer without its own block. |

A resin profile marked `extension = true` adds `per_printer` blocks to the
base profile of the same id instead of replacing it. That is how calibration
for a machine gets added to a resin someone else published.

At resolution time the base and its extension are composed into one
effective material (`EffectiveMaterial`), and `resolve` reads that composed
view. An extension may only add calibration pairs for printers the base does
not cover; everything else it might carry is excluded and reported as issue
values rather than errors: a file that is not flagged or that names a
different resin is ignored whole, a pair the base already covers is ignored
on its own (the extension's other pairs still apply), and values in the
inherited `[intent]` and `[defaults]` sections are ignored. An invalid
extension therefore never makes a valid printer and resin pairing unusable.

Some section names are reserved for planned features and are rejected at
parse time: `[instance]` on printers, and `[per_environment]`,
`[per_session_state]` and `[per_printer."<id>".constraints]` on resins.

## Resolution

Each value is taken from the first source that has it:

1. The effective material's `per_printer` block matching the printer's id
   (the base profile's own pairs plus those its extension adds).
2. Failing that, the effective material's `generic-machine` block, recorded
   as a warning. The printer's own block, when present, is used alone: its
   unset values do not fall through to the generic block.
3. The resin's `defaults`.
4. The resin's `intent`, where the value can be derived from it. The layer
   height comes from the target layer height; the normal exposure comes from
   the target energy density divided by the printer's light intensity, so it
   needs a printer that declares one.

`bottom_exposure_sec` has no derivation and must come from calibration or an
override.

The value is then checked against the printer's `constraints`, and depending
on the field it is quantised to a multiple, clamped into range, clamped at
the maximum, or rejected for being below the minimum. Overrides go through
the same pass, so they cannot exceed the printer's limits either. A required
value that no source supplies is an error naming the field.

`warnings` records what happened to each value: that it fell back to the
resin's defaults, that it was derived from intent, or that a constraint moved
it by more than a threshold; and, once per resolution, that the generic
machine's block stood in for the printer's own.

## Profile library

A profile library is one directory laid out as `printers/<id>.toml`,
`materials/<id>.toml` and `materials/extensions/<id>.toml`; the user's
library lives under a per-platform user data directory. Loading by id
searches the configured libraries in precedence order (`ProfileLibraries`:
the user library, then every bundled library) and reports every path it
tried when none has the profile. Material extensions are searched in the
user library only.

`save_printer_profile_to_library` and `save_material_profile_to_library`
write a profile into that layout by its id; `save_printer_image_to_library`
copies a preview image to `images/<id>.<ext>` and returns the reference to
store in the printer's `image_asset_path`; `delete_material_profile_from_library`
and `delete_material_extension_from_library` remove a resin profile or its
extension.

`export_package` writes a selection into a zip archive mirroring that layout,
and `import_package` installs one, reporting which profiles collide with
those already present.

For the API, see [docs.rs](https://docs.rs/sc-print-profiles).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.
