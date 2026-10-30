/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The held slice result and the re-slice decision.
//!
//! A completed slice is kept on the
//! [`SceneDocument`](crate::scene_document::SceneDocument) as a
//! [`SliceResult`]: the produced contours plus the [`SliceParams`] the run
//! used. When the Slice button is pressed again, the current inputs are
//! compared against the held ones through [`decide_re_slice`] to decide whether
//! the previous result is still current (nothing to do), stale (confirm before
//! overwriting), or absent (a first slice). A resin or printer edited in place
//! keeps its id, so the result also keeps the [`LayerValues`] its profiles
//! resolved to, and a press compares them against what the profiles resolve to
//! now.

use sc_print_glue::PlateContours;
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId, ScaleCompensation};
use sc_print_profiles::{
    EffectiveMaterial, PrinterProfile, resolve_layer_height, resolve_scale_compensation,
    snap_to_z_step,
};

use crate::print_selection::PrintSelectionInForce;
use crate::scene_document::SceneRevision;

/// The inputs a slice ran with: the printer and resin in force, the per-print
/// first-layer height (absent means the material's resolved layer height), the
/// anti-aliasing level, and an opaque marker of the scene geometry at slice
/// time.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SliceParams {
    pub printer: PrinterId,
    pub resin: MaterialId,
    pub first_layer_height: Option<PositiveFiniteMm>,
    pub aa_level: AaLevel,
    pub geometry: SceneRevision,
}

impl SliceParams {
    /// Returns the inputs that a slice of `selection` runs with, or `None`
    /// while it has no printer or no resin in force.
    pub(crate) fn for_selection(
        selection: &PrintSelectionInForce,
        aa_level: AaLevel,
        geometry: SceneRevision,
    ) -> Option<Self> {
        Some(Self {
            printer: selection.printer().in_force()?.clone(),
            resin: selection.resin().in_force()?.clone(),
            first_layer_height: selection.first_layer_height(),
            aa_level,
            geometry,
        })
    }
}

/// The values the printer and resin profiles resolve to that shape a slice's
/// layers: the layer height, the first-layer height once snapped to the
/// printer's z step, and the resin's scale compensation. Everything else in the
/// profiles (exposures, lifts, the panel) is read at export time and never
/// reaches the layers, so editing it keeps a held slice current.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerValues {
    pub layer_height: PositiveFiniteMm,
    pub first_layer_height: PositiveFiniteMm,
    pub scale: ScaleCompensation,
}

impl LayerValues {
    /// Resolves the values a slice of `printer` and `material` with the
    /// per-print `first_layer_height` is cut with, the way `slice_plate`
    /// resolves them, or `None` when the layer height does not resolve (such a
    /// pairing does not slice).
    pub(crate) fn resolve(
        printer: &PrinterProfile,
        material: &EffectiveMaterial,
        first_layer_height: Option<PositiveFiniteMm>,
    ) -> Option<Self> {
        let layer_height = resolve_layer_height(printer, material, None).ok()?;
        let first_layer_height = first_layer_height
            .map(|height| snap_to_z_step(&printer.constraints, height))
            .unwrap_or(layer_height);
        Some(Self {
            layer_height,
            first_layer_height,
            scale: resolve_scale_compensation(printer, material),
        })
    }
}

/// A completed slice held for the session: the merged plate contours with their
/// schedule and the validity token, captured off the event loop for
/// persistence, plus the parameters the run used.
#[derive(Clone, Debug)]
pub(crate) struct SliceResult {
    /// The whole plate's merged, world-coordinate contours, paired with the
    /// layer schedule they were cut on.
    pub contours: PlateContours,
    /// The slice-input fingerprint token, captured off the event loop and
    /// persisted so a later load can revalidate the slices.
    #[allow(dead_code)]
    pub token: String,
    pub params: SliceParams,
    /// The values the profiles resolved to when the layers were cut; `None`
    /// when unknown, which never counts as current.
    pub layers: Option<LayerValues>,
}

/// What a Slice press should do given the currently-held result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReSliceDecision {
    /// No result is held yet: slice.
    FirstSlice,
    /// The held result matches the current inputs: nothing to do.
    Unchanged,
    /// The inputs changed since the held result: re-slicing overwrites it.
    Changed,
}

/// Decides what a Slice press does. The scalar inputs are compared here;
/// `geometry_changed` carries the scene-geometry comparison, which
/// [`SceneDocument::slice_geometry_changed`][changed] answers from the edit
/// history, and `layers_changed` whether the profiles now resolve other
/// [`LayerValues`] than the held slice was cut with ([`layers_changed`]). Both
/// are only consulted when a result is held.
///
/// [changed]: crate::scene_document::SceneDocument::slice_geometry_changed
pub(crate) fn decide_re_slice(
    held: Option<&SliceParams>,
    current: &SliceParams,
    geometry_changed: bool,
    layers_changed: bool,
) -> ReSliceDecision {
    let Some(held) = held else {
        return ReSliceDecision::FirstSlice;
    };
    let scalars_match = held.printer == current.printer
        && held.resin == current.resin
        && held.first_layer_height == current.first_layer_height
        && held.aa_level == current.aa_level;
    if scalars_match && !geometry_changed && !layers_changed {
        ReSliceDecision::Unchanged
    } else {
        ReSliceDecision::Changed
    }
}

/// Whether a held slice cut with `held` no longer matches `current`, what the
/// profiles resolve to now. Unknown values on either side count as a change: a
/// slice that cannot be shown current is never kept as current.
pub(crate) fn layers_changed(held: Option<LayerValues>, current: Option<LayerValues>) -> bool {
    match (held, current) {
        (Some(held), Some(current)) => held != current,
        _ => true,
    }
}

#[cfg(test)]
mod decide_re_slice_tests {
    use super::*;

    fn params() -> SliceParams {
        SliceParams {
            printer: PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
            resin: MaterialId::new("standard-405").expect("valid id"),
            first_layer_height: Some(PositiveFiniteMm::new(0.05).expect("valid length")),
            aa_level: AaLevel::Off,
            geometry: SceneRevision::from_top_edit_id(Some(7)),
        }
    }

    #[test]
    fn no_held_result_is_a_first_slice() {
        assert_eq!(
            decide_re_slice(None, &params(), false, false),
            ReSliceDecision::FirstSlice
        );
    }

    #[test]
    fn matching_scalars_and_unchanged_geometry_is_unchanged() {
        let held = params();
        assert_eq!(
            decide_re_slice(Some(&held), &params(), false, false),
            ReSliceDecision::Unchanged
        );
    }

    #[test]
    fn changed_geometry_alone_is_a_change() {
        let held = params();
        assert_eq!(
            decide_re_slice(Some(&held), &params(), true, false),
            ReSliceDecision::Changed
        );
    }

    #[test]
    fn a_different_printer_is_a_change() {
        let held = params();
        let current = SliceParams {
            printer: PrinterId::new("anycubic-photon-mono-x").expect("valid id"),
            ..params()
        };
        assert_eq!(
            decide_re_slice(Some(&held), &current, false, false),
            ReSliceDecision::Changed
        );
    }

    #[test]
    fn a_different_resin_is_a_change() {
        let held = params();
        let current = SliceParams {
            resin: MaterialId::new("abs-like-405").expect("valid id"),
            ..params()
        };
        assert_eq!(
            decide_re_slice(Some(&held), &current, false, false),
            ReSliceDecision::Changed
        );
    }

    #[test]
    fn a_different_first_layer_height_is_a_change() {
        let held = params();
        let current = SliceParams {
            first_layer_height: Some(PositiveFiniteMm::new(0.1).expect("valid length")),
            ..params()
        };
        assert_eq!(
            decide_re_slice(Some(&held), &current, false, false),
            ReSliceDecision::Changed
        );
    }

    #[test]
    fn a_different_aa_level_is_a_change() {
        let held = params();
        let current = SliceParams {
            aa_level: AaLevel::X4,
            ..params()
        };
        assert_eq!(
            decide_re_slice(Some(&held), &current, false, false),
            ReSliceDecision::Changed
        );
    }

    #[test]
    fn a_profile_edited_in_place_is_a_change() {
        // Same ids, same scene: the resin's values moved under its id (#280).
        let held = params();
        assert_eq!(
            decide_re_slice(Some(&held), &params(), false, true),
            ReSliceDecision::Changed
        );
    }

    #[test]
    fn dropping_an_explicit_first_layer_height_is_a_change() {
        let held = params();
        let current = SliceParams {
            first_layer_height: None,
            ..params()
        };
        assert_eq!(
            decide_re_slice(Some(&held), &current, false, false),
            ReSliceDecision::Changed
        );
    }
}

#[cfg(test)]
mod layers_changed_tests {
    use super::*;

    use sc_print_primitives::ScaleCompensationPercent;

    fn values(scale_x: f64) -> LayerValues {
        LayerValues {
            layer_height: PositiveFiniteMm::new(0.05).expect("valid length"),
            first_layer_height: PositiveFiniteMm::new(0.05).expect("valid length"),
            scale: ScaleCompensation {
                x: ScaleCompensationPercent::new(scale_x).expect("in range"),
                ..ScaleCompensation::NONE
            },
        }
    }

    #[test]
    fn the_same_values_are_no_change() {
        assert!(!layers_changed(Some(values(0.0)), Some(values(0.0))));
    }

    #[test]
    fn other_values_are_a_change() {
        assert!(layers_changed(Some(values(0.0)), Some(values(1.0))));
    }

    #[test]
    fn unknown_values_on_either_side_are_a_change() {
        assert!(layers_changed(None, Some(values(0.0))));
        assert!(layers_changed(Some(values(0.0)), None));
        assert!(layers_changed(None, None));
    }
}

#[cfg(test)]
mod for_selection_tests {
    use super::*;

    use crate::print_selection::{PrintDefaults, PrintSelection};

    fn printer() -> PrinterId {
        PrinterId::new("elegoo-saturn-4-ultra").expect("valid id")
    }

    fn resin() -> MaterialId {
        MaterialId::new("standard-405").expect("valid id")
    }

    fn geometry() -> SceneRevision {
        SceneRevision::from_top_edit_id(Some(7))
    }

    #[test]
    fn no_printer_in_force_gives_no_params() {
        let selection = PrintSelection {
            resin: Some(resin()),
            ..PrintSelection::default()
        }
        .in_force(&PrintDefaults::default());

        assert_eq!(
            SliceParams::for_selection(&selection, AaLevel::Off, geometry()),
            None,
        );
    }

    #[test]
    fn no_resin_in_force_gives_no_params() {
        let selection = PrintSelection {
            printer: Some(printer()),
            ..PrintSelection::default()
        }
        .in_force(&PrintDefaults::default());

        assert_eq!(
            SliceParams::for_selection(&selection, AaLevel::Off, geometry()),
            None,
        );
    }

    #[test]
    fn a_printer_and_a_resin_in_force_give_every_field() {
        let height = PositiveFiniteMm::new(0.05).expect("valid length");
        let selection = PrintSelection {
            printer: Some(printer()),
            first_layer_height: Some(height),
            ..PrintSelection::default()
        }
        .in_force(&PrintDefaults {
            printer: None,
            resin: Some(resin()),
        });

        assert_eq!(
            SliceParams::for_selection(&selection, AaLevel::X4, geometry()),
            Some(SliceParams {
                printer: printer(),
                resin: resin(),
                first_layer_height: Some(height),
                aa_level: AaLevel::X4,
                geometry: geometry(),
            }),
        );
    }
}
