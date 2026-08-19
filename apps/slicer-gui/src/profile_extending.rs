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
use sc_print_profiles::{
    CalibrationTarget, EffectiveMaterial, MaterialDefaults, MaterialIntent, MaterialPerPrinter,
    MaterialProfile,
};
use slint::SharedString;

use crate::i18n::ctr;
use crate::profile_catalog::PrinterCatalogEntry;

/// The calibration targets the effective material does not cover yet:
/// every catalog printer (the generic machine included) without a
/// block in the composed base-plus-extension view. The extend dialog
/// lists exactly these, which is the single place the "only uncovered
/// targets" rule is enforced. Ordered like the editor's picker: the
/// default printer first, the generic machine next, the rest in
/// catalog order.
pub(crate) fn uncovered_targets(
    material: &EffectiveMaterial,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
) -> Vec<(CalibrationTarget, SharedString)> {
    let default_target = default_printer.map(CalibrationTarget::from_printer);
    let rank = |target: &CalibrationTarget| -> u8 {
        if default_target.as_ref() == Some(target) {
            0
        } else {
            match target {
                CalibrationTarget::Generic => 1,
                CalibrationTarget::Printer(_) => 2,
            }
        }
    };
    let mut entries: Vec<(CalibrationTarget, &PrinterCatalogEntry)> = printers
        .iter()
        .map(|printer| (CalibrationTarget::from_printer(&printer.id), printer))
        .filter(|(target, _)| !material.per_printer().contains_key(target))
        .collect();
    entries.sort_by_key(|(target, _)| rank(target));
    entries
        .into_iter()
        .map(|(target, printer)| {
            let label = match target {
                CalibrationTarget::Generic => ctr!("ExtendDialog", "Generic"),
                CalibrationTarget::Printer(_) => SharedString::from(printer.name.as_str()),
            };
            (target, label)
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
    target: &CalibrationTarget,
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
