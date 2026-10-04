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
//! overwriting), or absent (a first slice).

use sc_print_glue::PlateContours;
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};

use crate::scene_document::SceneRevision;

/// The effective inputs a slice ran with: the resolved printer and resin, the
/// per-print first-layer height (absent means the material's resolved layer
/// height), the anti-aliasing level, and an opaque marker of the scene geometry
/// at slice time.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SliceParams {
    pub printer: PrinterId,
    pub resin: MaterialId,
    pub first_layer_height: Option<PositiveFiniteMm>,
    pub aa_level: AaLevel,
    pub geometry: SceneRevision,
}

/// A completed slice held for the session: the merged plate contours with their
/// schedule and the validity token, captured off the event loop for
/// persistence, plus the parameters the run used.
#[derive(Clone, Debug)]
pub(crate) struct SliceResult {
    /// The whole plate's merged, world-coordinate contours, paired with the
    /// layer schedule they were cut on. Slice persistence and the layer view
    /// read them back; the write guard compares the schedule by value on save.
    pub contours: PlateContours,
    /// The slice-input fingerprint token, captured off the event loop and
    /// persisted so a later load can revalidate the slices.
    #[allow(dead_code)]
    pub token: String,
    pub params: SliceParams,
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

/// Decide what a Slice press should do. The scalar inputs are compared here;
/// `geometry_changed` carries the scene-geometry comparison, which the document
/// answers from its edit history (see
/// [`SceneDocument::slice_geometry_changed`](crate::scene_document::SceneDocument::slice_geometry_changed)).
/// It is only consulted when a result is held.
pub(crate) fn decide_re_slice(
    held: Option<&SliceParams>,
    current: &SliceParams,
    geometry_changed: bool,
) -> ReSliceDecision {
    let Some(held) = held else {
        return ReSliceDecision::FirstSlice;
    };
    let scalars_match = held.printer == current.printer
        && held.resin == current.resin
        && held.first_layer_height == current.first_layer_height
        && held.aa_level == current.aa_level;
    if scalars_match && !geometry_changed {
        ReSliceDecision::Unchanged
    } else {
        ReSliceDecision::Changed
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
            decide_re_slice(None, &params(), false),
            ReSliceDecision::FirstSlice
        );
    }

    #[test]
    fn matching_scalars_and_unchanged_geometry_is_unchanged() {
        let held = params();
        assert_eq!(
            decide_re_slice(Some(&held), &params(), false),
            ReSliceDecision::Unchanged
        );
    }

    #[test]
    fn changed_geometry_alone_is_a_change() {
        let held = params();
        assert_eq!(
            decide_re_slice(Some(&held), &params(), true),
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
            decide_re_slice(Some(&held), &current, false),
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
            decide_re_slice(Some(&held), &current, false),
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
            decide_re_slice(Some(&held), &current, false),
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
            decide_re_slice(Some(&held), &current, false),
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
            decide_re_slice(Some(&held), &current, false),
            ReSliceDecision::Changed
        );
    }
}
