/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! View model of the read-only resin profile viewer.
//!
//! Projects an [`EffectiveMaterial`] (the base profile with whatever
//! its extension adds) into the section list the profile view dialog
//! renders: identity, intent, defaults, then one section per
//! calibration block. Every optional field is rendered explicitly, an
//! unset one as the translated "not set" text, so a partial profile
//! reads as partial instead of hiding what is missing.

use std::collections::BTreeMap;

use sc_print_primitives::PrinterId;
use sc_print_profiles::{
    EffectiveMaterial, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialProfile, generic_printer_id,
};
use slint::{ModelRc, SharedString, VecModel};

use crate::i18n::ctr;
use crate::profile_catalog::{PrinterCatalogEntry, ProfileSource};
use crate::{ProfileViewField, ProfileViewSection};

/// Headline shown above the sections: brand plus display name.
pub(crate) fn profile_display_name(identity: &MaterialIdentity) -> String {
    format!("{} {}", identity.brand.as_str(), identity.name.as_str())
}

/// Build the viewer sections: identity, intent, defaults, then the
/// effective material's calibration blocks. The default printer's own
/// block comes first, the generic block next, the rest in
/// display-name order; the block the default printer would actually
/// use (its own, or the generic fallback) is highlighted.
pub(crate) fn build_profile_view_model(
    material: &EffectiveMaterial,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
    source: ProfileSource,
) -> Vec<ProfileViewSection> {
    let mut sections = vec![
        identity_section(material.identity(), source),
        intent_section(material.intent()),
        defaults_section(material.defaults()),
    ];
    sections.extend(calibration_sections(
        material.per_printer(),
        printers,
        default_printer,
    ));
    sections
}

fn field(label: SharedString, value: SharedString) -> ProfileViewField {
    ProfileViewField { label, value }
}

fn not_set() -> SharedString {
    ctr!("ProfileViewDialog", "not set")
}

fn fmt_f64(value: f64) -> SharedString {
    SharedString::from(format!("{value}"))
}

fn fmt_opt_f64(value: Option<f64>) -> SharedString {
    value.map(fmt_f64).unwrap_or_else(not_set)
}

fn fmt_opt_u32(value: Option<u32>) -> SharedString {
    value
        .map(|v| SharedString::from(format!("{v}")))
        .unwrap_or_else(not_set)
}

fn section(
    title: SharedString,
    highlighted: bool,
    fields: Vec<ProfileViewField>,
) -> ProfileViewSection {
    ProfileViewSection {
        title,
        highlighted,
        fields: ModelRc::new(VecModel::from(fields)),
    }
}

pub(crate) fn identity_section(
    identity: &MaterialIdentity,
    source: ProfileSource,
) -> ProfileViewSection {
    let origin = match source {
        ProfileSource::Bundled => ctr!("ProfileViewDialog", "Bundled"),
        ProfileSource::User => ctr!("ProfileViewDialog", "Custom"),
    };
    section(
        ctr!("ProfileViewDialog", "Identity"),
        false,
        vec![
            field(
                ctr!("ProfileViewDialog", "Brand"),
                SharedString::from(identity.brand.as_str()),
            ),
            field(
                ctr!("ProfileViewDialog", "Name"),
                SharedString::from(identity.name.as_str()),
            ),
            field(
                ctr!("ProfileViewDialog", "Family"),
                SharedString::from(identity.family.as_str()),
            ),
            field(
                ctr!("ProfileViewDialog", "Profile version"),
                SharedString::from(identity.profile_version.to_string()),
            ),
            field(ctr!("ProfileViewDialog", "Origin"), origin),
        ],
    )
}

pub(crate) fn intent_section(intent: &MaterialIntent) -> ProfileViewSection {
    section(
        ctr!("ProfileViewDialog", "Intent"),
        false,
        vec![
            field(
                ctr!("ProfileViewDialog", "Target energy density (mJ/cm2)"),
                fmt_opt_f64(intent.target_energy_density_mj_per_cm2.map(|v| v.value())),
            ),
            field(
                ctr!("ProfileViewDialog", "Target layer height (mm)"),
                fmt_opt_f64(intent.target_layer_height_mm.map(|v| v.value())),
            ),
        ],
    )
}

fn defaults_section(defaults: &MaterialDefaults) -> ProfileViewSection {
    let scale = defaults
        .scale_compensation_pct
        .map(|scale| {
            SharedString::from(format!(
                "{}% / {}% / {}%",
                scale.x.value(),
                scale.y.value(),
                scale.z.value(),
            ))
        })
        .unwrap_or_else(not_set);
    section(
        ctr!("ProfileViewDialog", "Defaults"),
        false,
        vec![
            field(
                ctr!("ProfileViewDialog", "Scale compensation (x/y/z)"),
                scale,
            ),
            field(
                ctr!("ProfileViewDialog", "Bottom layer count"),
                fmt_opt_u32(defaults.bottom_layer_count),
            ),
        ],
    )
}

/// The profile's own calibration blocks as `(printer id, label)`
/// picker entries for the editor, ordered like the viewer sections:
/// the default printer's block first, generic next, the rest by
/// display name. Labels are the printer names ("Generic" for the
/// generic block).
pub(crate) fn ordered_calibration_targets(
    profile: &MaterialProfile,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
) -> Vec<(PrinterId, SharedString)> {
    let display_label = |id: &PrinterId| -> String {
        printers
            .iter()
            .find(|printer| &printer.id == id)
            .map(|printer| printer.name.clone())
            .unwrap_or_else(|| id.as_str().to_owned())
    };
    let mut ids: Vec<&PrinterId> = profile.per_printer.keys().collect();
    ids.sort_by(|a, b| {
        let rank = |id: &PrinterId| -> u8 {
            if default_printer == Some(id) {
                0
            } else if id == generic_printer_id() {
                1
            } else {
                2
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| display_label(a).cmp(&display_label(b)))
    });
    ids.into_iter()
        .map(|id| {
            let label = if id == generic_printer_id() {
                ctr!("ProfileViewDialog", "Generic")
            } else {
                SharedString::from(display_label(id))
            };
            (id.clone(), label)
        })
        .collect()
}

fn calibration_sections(
    blocks: &BTreeMap<PrinterId, MaterialPerPrinter>,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
) -> Vec<ProfileViewSection> {
    let mut blocks: Vec<(&PrinterId, &MaterialPerPrinter)> = blocks.iter().collect();

    // The id whose block the default printer would use: its own when
    // present, otherwise the generic fallback.
    let highlight_id: Option<&PrinterId> = default_printer.and_then(|wanted| {
        if blocks.iter().any(|(id, _)| *id == wanted) {
            Some(wanted)
        } else if blocks.iter().any(|(id, _)| *id == generic_printer_id()) {
            Some(generic_printer_id())
        } else {
            None
        }
    });

    let display_label = |id: &PrinterId| -> String {
        printers
            .iter()
            .find(|printer| &printer.id == id)
            .map(|printer| printer.name.clone())
            .unwrap_or_else(|| id.as_str().to_owned())
    };

    // Default printer's block first, generic next, the rest by
    // display name.
    blocks.sort_by(|(a, _), (b, _)| {
        let rank = |id: &PrinterId| -> u8 {
            if default_printer == Some(id) {
                0
            } else if id == generic_printer_id() {
                1
            } else {
                2
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| display_label(a).cmp(&display_label(b)))
    });

    blocks
        .into_iter()
        .map(|(id, block)| {
            let title = if id == generic_printer_id() {
                ctr!("ProfileViewDialog", "Generic calibration")
            } else {
                ctr!("ProfileViewDialog", "Calibration: {}", display_label(id))
            };
            section(title, highlight_id == Some(id), calibration_fields(block))
        })
        .collect()
}

fn calibration_fields(block: &MaterialPerPrinter) -> Vec<ProfileViewField> {
    vec![
        field(
            ctr!("ProfileViewDialog", "Normal exposure (s)"),
            fmt_opt_f64(block.normal_exposure_sec.map(|v| v.value())),
        ),
        field(
            ctr!("ProfileViewDialog", "Bottom exposure (s)"),
            fmt_opt_f64(block.bottom_exposure_sec.map(|v| v.value())),
        ),
        field(
            ctr!("ProfileViewDialog", "Bottom layer count"),
            fmt_opt_u32(block.bottom_layer_count),
        ),
        field(
            ctr!("ProfileViewDialog", "Lift distance (mm)"),
            fmt_opt_f64(block.lift_distance_mm.map(|v| v.value())),
        ),
        field(
            ctr!("ProfileViewDialog", "Lift speed (mm/min)"),
            fmt_opt_f64(block.lift_speed_mm_min.map(|v| v.value())),
        ),
        field(
            ctr!("ProfileViewDialog", "Retract speed (mm/min)"),
            fmt_opt_f64(block.retract_speed_mm_min.map(|v| v.value())),
        ),
    ]
}

#[cfg(test)]
mod tests;
