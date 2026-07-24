/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Assembling a resin profile extension update.
//!
//! Extending adds a calibration block for a target the resin does not
//! cover yet. The block goes into the resin's same-id extension file:
//! created on the first extend (identity copied verbatim from the
//! base, extension flag set, intent and defaults left empty since
//! they are inherited from the base), appended to on later ones. The
//! dialog wiring lives in the preferences window.

use std::collections::BTreeMap;

use sc_print_primitives::PrinterId;
use sc_print_profiles::{MaterialDefaults, MaterialIntent, MaterialPerPrinter, MaterialProfile};
use slint::SharedString;

use crate::i18n::ctr;
use crate::profile_catalog::{GENERIC_PRINTER_ID, PrinterCatalogEntry};

/// The calibration targets not yet covered by the base profile or its
/// extension: every catalog printer (the generic machine included)
/// without a block in either file. The extend dialog lists exactly
/// these, which is the single place the "only uncovered targets" rule
/// is enforced. Ordered like the editor's picker: the default printer
/// first, the generic machine next, the rest in catalog order.
pub(crate) fn uncovered_targets(
    base: &MaterialProfile,
    extension: Option<&MaterialProfile>,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
) -> Vec<(PrinterId, SharedString)> {
    let covered = |id: &PrinterId| {
        base.per_printer.contains_key(id)
            || extension.is_some_and(|ext| ext.per_printer.contains_key(id))
    };
    let rank = |id: &PrinterId| -> u8 {
        if default_printer == Some(id) {
            0
        } else if id.as_str() == GENERIC_PRINTER_ID {
            1
        } else {
            2
        }
    };
    let mut entries: Vec<&PrinterCatalogEntry> = printers
        .iter()
        .filter(|printer| !covered(&printer.id))
        .collect();
    entries.sort_by_key(|printer| rank(&printer.id));
    entries
        .into_iter()
        .map(|printer| {
            let label = if printer.id.as_str() == GENERIC_PRINTER_ID {
                ctr!("ExtendDialog", "Generic")
            } else {
                SharedString::from(printer.name.as_str())
            };
            (printer.id.clone(), label)
        })
        .collect()
}

/// The extension profile to write after adding a block for `target`.
/// Appends to `existing_extension` when the resin already has one;
/// otherwise starts a fresh extension from the base's identity. A base
/// that is itself an extension file (the standalone row of an orphaned
/// extension) is appended to directly, so its existing pairs survive.
pub(crate) fn build_extension_update(
    base: &MaterialProfile,
    existing_extension: Option<&MaterialProfile>,
    target: &PrinterId,
    values: MaterialPerPrinter,
) -> MaterialProfile {
    let mut extension = match existing_extension {
        Some(extension) => extension.clone(),
        None if base.extension => base.clone(),
        None => MaterialProfile {
            extension: true,
            identity: base.identity.clone(),
            intent: MaterialIntent {
                target_energy_density_mj_per_cm2: None,
                target_layer_height_mm: None,
            },
            defaults: MaterialDefaults {
                scale_compensation_pct: None,
                bottom_layer_count: None,
            },
            per_printer: BTreeMap::new(),
        },
    };
    extension.per_printer.insert(target.clone(), values);
    extension
}

#[cfg(test)]
mod tests;
