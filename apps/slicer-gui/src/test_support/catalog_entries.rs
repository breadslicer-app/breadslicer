/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use sc_print_primitives::{
    BuildVolume, FormatKind, NonEmptyString, OutputFormatBinding, OutputFormatBindings, PrinterId,
};

use crate::profile_catalog::{PrinterCatalogEntry, ProfileSource};

/// Returns a bundled printer entry with an empty name, the Goo v3.0 output
/// format and a plate of `[x, y, z]` millimetres.
pub(crate) fn printer_entry(
    id: PrinterId,
    manufacturer: &str,
    plate: [f64; 3],
) -> PrinterCatalogEntry {
    let [x, y, z] = plate;
    PrinterCatalogEntry {
        id,
        manufacturer: manufacturer.to_owned(),
        name: String::new(),
        build_volume: BuildVolume::new(x, y, z).expect("valid build volume"),
        output_formats: OutputFormatBindings::new(OutputFormatBinding {
            format: FormatKind::Goo,
            version: Some(NonEmptyString::new("v3.0").expect("non-empty version")),
        }),
        image_path: None,
        source: ProfileSource::Bundled,
    }
}
