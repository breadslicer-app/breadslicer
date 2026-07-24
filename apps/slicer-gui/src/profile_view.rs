/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! View model of the read-only resin profile viewer.
//!
//! Projects a loaded [`MaterialProfile`] (plus its extension profile,
//! when one exists) into the section list the profile view dialog
//! renders: identity, intent, defaults, then one section per
//! calibration block. Every optional field is rendered explicitly, an
//! unset one as the translated "not set" text, so a partial profile
//! reads as partial instead of hiding what is missing.

use sc_print_primitives::PrinterId;
use sc_print_profiles::{MaterialPerPrinter, MaterialProfile};
use slint::{ModelRc, SharedString, VecModel};

use crate::i18n::ctr;
use crate::profile_catalog::{GENERIC_PRINTER_ID, PrinterCatalogEntry, ProfileSource};
use crate::{ProfileViewField, ProfileViewSection};

/// Headline shown above the sections: brand plus display name.
pub(crate) fn profile_display_name(profile: &MaterialProfile) -> String {
    format!(
        "{} {}",
        profile.identity.brand.as_str(),
        profile.identity.name.as_str(),
    )
}

/// Build the viewer sections: identity, intent, defaults, then the
/// calibration blocks of the base profile and its extension. The
/// default printer's own block comes first, the generic block next,
/// the rest in display-name order; the block the default printer
/// would actually use (its own, or the generic fallback) is
/// highlighted.
pub(crate) fn build_profile_view_model(
    profile: &MaterialProfile,
    extension: Option<&MaterialProfile>,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
    source: ProfileSource,
) -> Vec<ProfileViewSection> {
    let mut sections = vec![
        identity_section(profile, source),
        intent_section(profile),
        defaults_section(profile),
    ];
    sections.extend(calibration_sections(
        profile,
        extension,
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

fn identity_section(profile: &MaterialProfile, source: ProfileSource) -> ProfileViewSection {
    let identity = &profile.identity;
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

fn intent_section(profile: &MaterialProfile) -> ProfileViewSection {
    let intent = &profile.intent;
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

fn defaults_section(profile: &MaterialProfile) -> ProfileViewSection {
    let defaults = &profile.defaults;
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

/// One `(printer id, block)` pair per calibration block, from the base
/// profile and its extension (disjoint by construction: extensions
/// never override existing blocks).
fn collect_blocks<'a>(
    profile: &'a MaterialProfile,
    extension: Option<&'a MaterialProfile>,
) -> Vec<(&'a PrinterId, &'a MaterialPerPrinter)> {
    profile
        .per_printer
        .iter()
        .chain(extension.into_iter().flat_map(|ext| ext.per_printer.iter()))
        .collect()
}

fn calibration_sections(
    profile: &MaterialProfile,
    extension: Option<&MaterialProfile>,
    printers: &[PrinterCatalogEntry],
    default_printer: Option<&PrinterId>,
) -> Vec<ProfileViewSection> {
    let mut blocks = collect_blocks(profile, extension);

    // The id whose block the default printer would use: its own when
    // present, otherwise the generic fallback.
    let highlight_id: Option<&str> = default_printer.and_then(|wanted| {
        if blocks.iter().any(|(id, _)| *id == wanted) {
            Some(wanted.as_str())
        } else if blocks
            .iter()
            .any(|(id, _)| id.as_str() == GENERIC_PRINTER_ID)
        {
            Some(GENERIC_PRINTER_ID)
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
            } else if id.as_str() == GENERIC_PRINTER_ID {
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
            let title = if id.as_str() == GENERIC_PRINTER_ID {
                ctr!("ProfileViewDialog", "Generic calibration")
            } else {
                ctr!("ProfileViewDialog", "Calibration: {}", display_label(id))
            };
            section(
                title,
                highlight_id == Some(id.as_str()),
                calibration_fields(block),
            )
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
