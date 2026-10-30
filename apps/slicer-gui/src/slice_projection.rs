/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Projecting the chosen printer/resin and per-print settings onto the shared
//! slicing service.
//!
//! - [`load_selected_profiles`] resolves the printer and resin ids to full
//!   profiles across the installed libraries (the only I/O here).
//! - [`resolve_current_schedule`] derives the layer schedule the current
//!   selection would slice with, for save-time comparisons against a held
//!   slice.
//! - [`current_layer_values`] resolves the values that shape the layers, which
//!   a Slice press compares against the held slice's (#280).
//! - The slice run itself goes through sc-print-glue's `slice_plate`.

use std::fmt;

use sc_print_glue::{
    SliceInputFingerprintError, SlicePlateError, SlicingParams, UnschedulableHeight,
    UnsliceableScene, UnsliceableSelection, schedule_for_selection,
};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_print_profiles::{
    EffectiveMaterial, LoadMaterialProfileByIdError, LoadPrinterProfileByIdError, PrinterProfile,
    ProfileLibraries, ResolveError,
};
use sc_scene::Scene;
use sc_scene::error::SceneError;
use sc_sl1cer::error::SliceSceneError;
use sc_sl1cer::profile::LayerSchedule;

use crate::i18n::ctr;
use crate::slice_result::{LayerValues, SliceParams};

/// Why a slice run could not be prepared or completed. Carries the specific
/// failure so the UI can surface a meaningful message. Its text is in the
/// language in force each time it is formatted.
#[derive(Debug)]
pub(crate) enum SliceFailure {
    /// The selected printer profile could not be loaded.
    PrinterProfile(LoadPrinterProfileByIdError),
    /// The selected resin profile could not be loaded.
    MaterialProfile(LoadMaterialProfileByIdError),
    /// The scene's bounding box could not be computed.
    SceneAabb(SceneError),
    /// The scene contributes no geometry to slice.
    EmptySceneGeometry,
    /// The scene's top is at or below the build plate (z = 0), so nothing sits
    /// above it to slice.
    SceneBelowPlate { z_max: f64 },
    /// The print configuration could not be resolved.
    Resolve(ResolveError),
    /// The scene height does not yield a schedulable layer count.
    Projection(UnschedulableHeight),
    /// The slicing engine rejected the scene.
    Slice(SliceSceneError),
    /// The per-leaf slice stacks disagreed on their layer grid and could not be
    /// merged into 1 plate stack. Unreachable for leaves sliced under 1 shared
    /// schedule, but handled rather than panicked.
    SliceMerge,
    /// The slice-input fingerprint could not be computed.
    Fingerprint(SliceInputFingerprintError),
    /// The slicing worker panicked. Carries the panic message.
    Panicked(String),
}

impl fmt::Display for SliceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::PrinterProfile(err) => {
                ctr!("ErrorDialog", "Could not load the printer profile: {}", err)
            }
            Self::MaterialProfile(err) => {
                ctr!("ErrorDialog", "Could not load the resin profile: {}", err)
            }
            Self::SceneAabb(err) => {
                ctr!("ErrorDialog", "Could not measure the scene bounds: {}", err)
            }
            Self::EmptySceneGeometry => {
                ctr!("ErrorDialog", "The scene has no geometry to slice.")
            }
            Self::SceneBelowPlate { z_max } => ctr!(
                "ErrorDialog",
                "The scene's top is at z = {} mm; nothing sits above the build plate (z = 0). Move the model above the plate before slicing.",
                z_max
            ),
            Self::Resolve(err) => ctr!(
                "ErrorDialog",
                "Could not resolve the print configuration: {}",
                err
            ),
            Self::Projection(err) => {
                ctr!("ErrorDialog", "Could not derive the layer count: {}", err)
            }
            Self::Slice(err) => ctr!("ErrorDialog", "Slicing failed: {}", err),
            Self::SliceMerge => ctr!(
                "ErrorDialog",
                "The slice layers could not be merged (internal layer-grid mismatch)."
            ),
            Self::Fingerprint(err) => ctr!(
                "ErrorDialog",
                "Could not fingerprint the slice inputs: {}",
                err
            ),
            Self::Panicked(msg) => ctr!("ErrorDialog", "The slicer panicked: {}", msg),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for SliceFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PrinterProfile(err) => Some(err),
            Self::MaterialProfile(err) => Some(err),
            Self::SceneAabb(err) => Some(err),
            Self::Resolve(err) => Some(err),
            Self::Projection(err) => Some(err),
            Self::Slice(err) => Some(err),
            Self::Fingerprint(err) => Some(err),
            Self::EmptySceneGeometry
            | Self::SceneBelowPlate { .. }
            | Self::SliceMerge
            | Self::Panicked(_) => None,
        }
    }
}

impl From<UnsliceableScene> for SliceFailure {
    fn from(err: UnsliceableScene) -> Self {
        match err {
            UnsliceableScene::Aabb(err) => Self::SceneAabb(err),
            UnsliceableScene::NoGeometry => Self::EmptySceneGeometry,
            UnsliceableScene::TopAtOrBelowPlate { z_max_mm } => {
                Self::SceneBelowPlate { z_max: z_max_mm }
            }
            UnsliceableScene::Unschedulable(err) => Self::Projection(err),
        }
    }
}

impl From<UnsliceableSelection> for SliceFailure {
    fn from(err: UnsliceableSelection) -> Self {
        match err {
            UnsliceableSelection::LayerHeight(err) => Self::Resolve(err),
            UnsliceableSelection::Scene(err) => err.into(),
        }
    }
}

impl From<SlicePlateError> for SliceFailure {
    fn from(err: SlicePlateError) -> Self {
        match err {
            SlicePlateError::Prepare(err) => err.into(),
            SlicePlateError::Engine(err) => Self::Slice(err),
        }
    }
}

/// Load the printer profile and the effective material for `printer` and
/// `resin` from the installed profile libraries. A problem with the resin's
/// extension never fails the call: the function prints a warning to stderr and
/// leaves the offending content out of the returned material.
pub(crate) fn load_selected_profiles(
    printer: &PrinterId,
    resin: &MaterialId,
) -> Result<(PrinterProfile, EffectiveMaterial), SliceFailure> {
    let libraries = ProfileLibraries::platform_default();
    let printer_profile = libraries
        .load_printer_by_id(printer)
        .map_err(SliceFailure::PrinterProfile)?;
    let (material, issues) = libraries
        .effective_material_by_id(resin)
        .map_err(SliceFailure::MaterialProfile)?;
    for issue in &issues {
        eprintln!("warning: {issue}");
    }
    Ok((printer_profile, material))
}

/// Resolve the [`LayerValues`] a slice with `params` would be cut with, from
/// the profiles on disk now, or `None` when they do not load or resolve.
pub(crate) fn current_layer_values(params: &SliceParams) -> Option<LayerValues> {
    let (printer, resin) = load_selected_profiles(&params.printer, &params.resin).ok()?;
    LayerValues::resolve(&printer, &resin, params.first_layer_height)
}

/// Resolve the layer schedule that slicing `scene` with `printer`, `resin`, and
/// `first_layer_height` would use.
pub(crate) fn resolve_current_schedule(
    printer: &PrinterId,
    resin: &MaterialId,
    first_layer_height: Option<PositiveFiniteMm>,
    scene: &Scene,
) -> Result<LayerSchedule, SliceFailure> {
    let (printer_profile, resin_profile) = load_selected_profiles(printer, resin)?;
    schedule_for_profiles(&printer_profile, &resin_profile, first_layer_height, scene)
}

/// [`resolve_current_schedule`] for profiles already loaded.
pub(crate) fn schedule_for_profiles(
    printer: &PrinterProfile,
    resin: &EffectiveMaterial,
    first_layer_height: Option<PositiveFiniteMm>,
    scene: &Scene,
) -> Result<LayerSchedule, SliceFailure> {
    let params = SlicingParams {
        layer_height_request: None,
        first_layer_height,
        // AA does not affect the schedule; any value serves.
        aa_level: AaLevel::Off,
    };
    schedule_for_selection(scene, printer, resin, &params).map_err(SliceFailure::from)
}

#[cfg(test)]
mod unsliceable_scene_mapping_tests {
    use super::*;

    use sc_mesh_core::transform::Transform3D;

    // The scene-derivation rejections themselves (no geometry, at or below the
    // plate, unschedulable heights) are pinned in sc-print-glue's own suite;
    // these tests pin the mapping onto the GUI's failure vocabulary.

    #[test]
    fn aabb_errors_map_to_scene_aabb() {
        let id = Scene::new().add_group(None, Transform3D::Identity);
        let failure = SliceFailure::from(UnsliceableScene::Aabb(SceneError::DanglingNodeId { id }));
        assert!(matches!(failure, SliceFailure::SceneAabb(_)));
    }

    #[test]
    fn no_geometry_maps_to_empty_scene_geometry() {
        let failure = SliceFailure::from(UnsliceableScene::NoGeometry);
        assert!(matches!(failure, SliceFailure::EmptySceneGeometry));
    }

    #[test]
    fn top_at_or_below_plate_maps_to_scene_below_plate_with_the_height() {
        let failure = SliceFailure::from(UnsliceableScene::TopAtOrBelowPlate { z_max_mm: -1.5 });
        assert!(matches!(
            failure,
            SliceFailure::SceneBelowPlate { z_max } if z_max == -1.5
        ));
    }

    #[test]
    fn unschedulable_heights_map_to_projection() {
        let failure = SliceFailure::from(UnsliceableScene::Unschedulable(
            UnschedulableHeight::NonFiniteHeight { z_max_mm: f64::NAN },
        ));
        assert!(matches!(failure, SliceFailure::Projection(_)));
    }

    #[test]
    fn an_unresolvable_layer_height_maps_to_resolve() {
        use sc_print_profiles::ResolvedField;

        let failure = SliceFailure::from(UnsliceableSelection::LayerHeight(
            ResolveError::MissingRequiredField {
                field: ResolvedField::LayerHeightMm,
            },
        ));
        assert!(matches!(failure, SliceFailure::Resolve(_)));
    }

    #[test]
    fn a_selection_scene_failure_delegates_to_the_scene_mapping() {
        let failure = SliceFailure::from(UnsliceableSelection::Scene(UnsliceableScene::NoGeometry));
        assert!(matches!(failure, SliceFailure::EmptySceneGeometry));
    }
}

#[cfg(test)]
mod slice_failure_text_tests {
    use super::*;

    use sc_mesh_core::transform::Transform3D;
    use sc_print_glue::SliceInputFingerprintError;
    use sc_print_profiles::ResolvedField;

    #[test]
    fn a_profile_that_does_not_load_names_the_profile_kind() {
        let printer = SliceFailure::PrinterProfile(LoadPrinterProfileByIdError::NotFoundAnywhere {
            attempted: Vec::new(),
        });
        assert_eq!(
            printer.to_string(),
            "Could not load the printer profile: printer profile not found in any of the \
             configured sources (no profile library roots configured)"
        );
        let resin = SliceFailure::MaterialProfile(LoadMaterialProfileByIdError::NotFoundAnywhere {
            attempted: Vec::new(),
        });
        assert_eq!(
            resin.to_string(),
            "Could not load the resin profile: material profile not found in any of the \
             configured sources (no profile library roots configured)"
        );
    }

    #[test]
    fn scene_failures_say_what_the_scene_lacks() {
        let id = Scene::new().add_group(None, Transform3D::Identity);
        assert_eq!(
            SliceFailure::SceneAabb(SceneError::DanglingNodeId { id }).to_string(),
            format!(
                "Could not measure the scene bounds: dangling node id {}",
                id.get()
            )
        );
        assert_eq!(
            SliceFailure::EmptySceneGeometry.to_string(),
            "The scene has no geometry to slice."
        );
        assert_eq!(
            SliceFailure::SceneBelowPlate { z_max: -1.5 }.to_string(),
            "The scene's top is at z = -1.5 mm; nothing sits above the build plate (z = 0). \
             Move the model above the plate before slicing."
        );
    }

    #[test]
    fn configuration_failures_name_the_step_that_failed() {
        assert_eq!(
            SliceFailure::Resolve(ResolveError::MissingRequiredField {
                field: ResolvedField::LayerHeightMm,
            })
            .to_string(),
            "Could not resolve the print configuration: required field 'layer_height_mm' has \
             no value after all resolution layers"
        );
        assert_eq!(
            SliceFailure::Projection(UnschedulableHeight::NonFiniteHeight { z_max_mm: f64::NAN })
                .to_string(),
            "Could not derive the layer count: the scene height z = NaN mm is not a finite value"
        );
    }

    #[test]
    fn run_failures_name_the_step_that_failed() {
        let id = Scene::new().add_group(None, Transform3D::Identity);
        assert_eq!(
            SliceFailure::Slice(SliceSceneError::EmptyScene).to_string(),
            "Slicing failed: scene has no mesh leaves to slice"
        );
        assert_eq!(
            SliceFailure::SliceMerge.to_string(),
            "The slice layers could not be merged (internal layer-grid mismatch)."
        );
        assert_eq!(
            SliceFailure::Fingerprint(SliceInputFingerprintError::Scene(
                SceneError::DanglingNodeId { id }
            ))
            .to_string(),
            format!(
                "Could not fingerprint the slice inputs: slice-input fingerprint: dangling node \
                 id {}",
                id.get()
            )
        );
        assert_eq!(
            SliceFailure::Panicked("boom".to_string()).to_string(),
            "The slicer panicked: boom"
        );
    }
}
