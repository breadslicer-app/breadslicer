/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Projecting the scene plus the chosen printer/resin and per-print
//! settings onto a slice profile.
//!
//! Two pieces: [`load_selected_profiles`] resolves the printer and
//! resin ids to full profiles on disk (the only I/O here), and
//! [`build_slice_profile`] resolves the layer height and hands the
//! scene to sc-print-glue's shared derivation, which measures the
//! model, derives the layer count, and projects the printer hardware
//! onto a [`SliceProfile`]. The GUI slices the live scene as
//! arranged; the only placement rule is that a scene with no geometry,
//! or one whose top is at or below the build plate (z = 0), cannot be
//! sliced.

use std::fmt;

use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::error::SceneError;
use sc_print_glue::{
    SliceInputFingerprintError, UnschedulableHeight, UnsliceableScene, UnsliceableSelection,
    slice_profile_for_selection,
};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_print_profiles::{
    EffectiveMaterial, LoadMaterialProfileByIdError, LoadPrinterProfileByIdError, PrinterProfile,
    ResolveError, UserLibraryDirError, compose_with_library_extension, load_material_profile_by_id,
    load_printer_profile_by_id, user_library_dir,
};
use sc_sl1cer::error::SliceSceneError;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

/// Why a slice run could not be prepared or completed. Carries the
/// specific failure so the UI can surface a meaningful message.
#[derive(Debug)]
pub(crate) enum SliceFailure {
    /// The user profile library directory could not be resolved.
    UserLibraryDir(UserLibraryDirError),
    /// The selected printer profile could not be loaded.
    PrinterProfile(LoadPrinterProfileByIdError),
    /// The selected resin profile could not be loaded.
    MaterialProfile(LoadMaterialProfileByIdError),
    /// The scene's bounding box could not be computed.
    SceneAabb(SceneError),
    /// The scene contributes no geometry to slice.
    EmptySceneGeometry,
    /// The scene's top is at or below the build plate (z = 0), so
    /// nothing sits above it to slice.
    SceneBelowPlate { z_max: f64 },
    /// The print configuration could not be resolved.
    Resolve(ResolveError),
    /// The scene height does not yield a schedulable layer count.
    Projection(UnschedulableHeight),
    /// The slicing engine rejected the scene.
    Slice(SliceSceneError),
    /// The per-leaf slice stacks disagreed on their layer grid and could
    /// not be merged into one plate stack. Unreachable for leaves sliced
    /// under one shared schedule, but handled rather than panicked.
    SliceMerge,
    /// The slice-input fingerprint could not be computed.
    Fingerprint(SliceInputFingerprintError),
    /// The slicing worker panicked. Carries the panic message.
    Panicked(String),
}

impl fmt::Display for SliceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserLibraryDir(err) => {
                write!(f, "Could not locate the profile library: {err}")
            }
            Self::PrinterProfile(err) => {
                write!(f, "Could not load the printer profile: {err}")
            }
            Self::MaterialProfile(err) => {
                write!(f, "Could not load the resin profile: {err}")
            }
            Self::SceneAabb(err) => write!(f, "Could not measure the scene bounds: {err}"),
            Self::EmptySceneGeometry => write!(f, "The scene has no geometry to slice."),
            Self::SceneBelowPlate { z_max } => write!(
                f,
                "The scene's top is at z = {z_max} mm; nothing sits above the build plate (z = 0). Move the model above the plate before slicing.",
            ),
            Self::Resolve(err) => {
                write!(f, "Could not resolve the print configuration: {err}")
            }
            Self::Projection(err) => write!(f, "Could not derive the layer count: {err}"),
            Self::Slice(err) => write!(f, "Slicing failed: {err}"),
            Self::SliceMerge => write!(
                f,
                "The slice layers could not be merged (internal layer-grid mismatch).",
            ),
            Self::Fingerprint(err) => {
                write!(f, "Could not fingerprint the slice inputs: {err}")
            }
            Self::Panicked(msg) => write!(f, "The slicer panicked: {msg}"),
        }
    }
}

impl std::error::Error for SliceFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UserLibraryDir(err) => Some(err),
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

/// Load the full printer and resin profiles named by their ids, using
/// the same roots the catalog walks: the user library first, then the
/// bundled profiles. The resin's same-id extension from the user
/// library (the only root that holds extensions) is composed in;
/// extension content the composition excludes is reported on stderr
/// and never fails the load.
pub(crate) fn load_selected_profiles(
    printer: &PrinterId,
    resin: &MaterialId,
) -> Result<(PrinterProfile, EffectiveMaterial), SliceFailure> {
    let user_lib = user_library_dir().map_err(SliceFailure::UserLibraryDir)?;
    let bundled = sc_app_paths::bundled_data_dir().join("profiles");
    let printer_profile = load_printer_profile_by_id(printer, None, &user_lib, &bundled)
        .map_err(SliceFailure::PrinterProfile)?;
    let resin_profile = load_material_profile_by_id(resin, None, &user_lib, &bundled)
        .map_err(SliceFailure::MaterialProfile)?;
    let (material, issues) = compose_with_library_extension(resin_profile, &user_lib);
    for issue in &issues {
        eprintln!("warning: {issue}");
    }
    Ok((printer_profile, material))
}

/// Build a [`SliceProfile`] for the current scene through the shared
/// selection entry: the layer height is the only material parameter
/// geometry slicing needs (the exposure and motion parameters belong
/// to the SLA output step), and the scene measurement, first-layer
/// fallback, layer-count derivation and hardware projection all live
/// in [`slice_profile_for_selection`]. No per-print layer-height
/// request is wired here yet, so `None` is passed.
pub(crate) fn build_slice_profile(
    printer: &PrinterProfile,
    resin: &EffectiveMaterial,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    scene: &Scene,
) -> Result<SliceProfile, SliceFailure> {
    slice_profile_for_selection(
        scene,
        printer,
        resin,
        None,
        first_layer_height,
        aa_level,
        PackingMode::None,
    )
    .map_err(SliceFailure::from)
}

/// Resolve the layer schedule for the current scene and selection the
/// same way [`build_slice_profile`] does, so a save-time comparison
/// against a held slice's schedule is apples to apples. The
/// anti-aliasing level is a raster parameter with no effect on the
/// schedule, so it is fixed here rather than threaded in.
pub(crate) fn resolve_current_schedule(
    printer: &PrinterId,
    resin: &MaterialId,
    first_layer_height: Option<PositiveFiniteMm>,
    scene: &Scene,
) -> Result<LayerSchedule, SliceFailure> {
    let (printer_profile, resin_profile) = load_selected_profiles(printer, resin)?;
    let profile = build_slice_profile(
        &printer_profile,
        &resin_profile,
        first_layer_height,
        AaLevel::Off,
        scene,
    )?;
    Ok(*profile.layer_schedule())
}

#[cfg(test)]
mod unsliceable_scene_mapping_tests {
    use super::*;

    use sc_mesh_core::transform::Transform3D;

    // The scene-derivation rejections themselves (no geometry, at or
    // below the plate, unschedulable heights) are pinned in
    // sc-print-glue's own suite; these tests pin the mapping onto the
    // GUI's failure vocabulary.

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
mod build_slice_profile_tests {
    use super::*;

    use std::sync::Arc;

    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::node::LeafPayload;
    use sc_mesh_core::transform::Transform3D;
    use sc_sl1cer::profile::LayerSchedule;

    const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-saturn-4-ultra"
manufacturer = "ELEGOO"
family = "Saturn"
name = "Saturn 4 Ultra"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
z_step_mm = 0.01
light_intensity_mw_per_cm2 = 5.0
"#;

    // A generic material with no per-printer exposure calibration: full
    // `resolve` would reject it, but geometry slicing needs only its
    // layer height, so `build_slice_profile` must still succeed.
    const MATERIAL_TOML: &str = r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[defaults.scale_compensation_pct]
x = 0.0
y = 0.0
z = 0.0

[per_printer]
"#;

    fn fixtures() -> (PrinterProfile, EffectiveMaterial) {
        let material: sc_print_profiles::MaterialProfile =
            toml::from_str(MATERIAL_TOML).expect("material fixture parses");
        (
            toml::from_str(PRINTER_TOML).expect("printer fixture parses"),
            EffectiveMaterial::from(material),
        )
    }

    fn cube_scene_top(z_max: f32) -> Scene {
        let v = |x, y, z| Vertex::new([x, y, z]);
        let vertices = vec![
            v(-1.0, -1.0, 0.0),
            v(1.0, -1.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(-1.0, 1.0, 0.0),
            v(-1.0, -1.0, z_max),
            v(1.0, -1.0, z_max),
            v(1.0, 1.0, z_max),
            v(-1.0, 1.0, z_max),
        ];
        let n = Normal::new([0.0, 0.0, 1.0]);
        let faces = vec![
            IndexedTriangle::from_raw_indices(n, [0, 1, 2]),
            IndexedTriangle::from_raw_indices(n, [0, 2, 3]),
        ];
        let mesh = IndexedMesh {
            meta: None,
            vertices,
            faces,
        };
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group as root");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        g
    }

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).expect("valid length")
    }

    // The first-layer fallback and layer-count arithmetic moved to
    // sc-print-glue with the shared derivation; its suite pins them.
    // What stays here is the delegation surface: the resolved layer
    // height feeds the derivation, failures map onto SliceFailure,
    // and slicing never requires the full print calibration.

    #[test]
    fn the_resolved_layer_height_drives_the_derivation() {
        // Layer height 0.05 comes from the material intent; z_max
        // 0.97 then yields 20 layers (pinned arithmetic in the glue
        // suite). This pins that build_slice_profile feeds the
        // RESOLVED height into the shared derivation.
        let (printer, resin) = fixtures();
        let profile =
            build_slice_profile(&printer, &resin, None, AaLevel::Off, &cube_scene_top(0.97))
                .expect("valid projection");
        match profile.layer_schedule() {
            LayerSchedule::Uniform(uniform) => {
                assert_eq!(uniform.layer_height().value(), 0.05);
                assert_eq!(uniform.total_layers().get(), 20);
            }
        }
    }

    #[test]
    fn the_first_layer_choice_reaches_the_derivation() {
        let (printer, resin) = fixtures();
        let profile = build_slice_profile(
            &printer,
            &resin,
            Some(mm(0.15)),
            AaLevel::Off,
            &cube_scene_top(0.97),
        )
        .expect("valid projection");
        match profile.layer_schedule() {
            LayerSchedule::Uniform(uniform) => {
                assert_eq!(uniform.first_layer_height().value(), 0.15);
            }
        }
    }

    #[test]
    fn an_empty_scene_is_rejected() {
        let (printer, resin) = fixtures();
        let err = build_slice_profile(&printer, &resin, None, AaLevel::Off, &Scene::new())
            .expect_err("an empty scene cannot be sliced");
        assert!(matches!(err, SliceFailure::EmptySceneGeometry));
    }

    #[test]
    fn a_material_without_exposure_calibration_can_still_be_sliced() {
        // Regression: geometry slicing must not require the exposure and
        // motion calibration that a full print-config resolution demands.
        // The fixture pairs a printer with a generic material that has no
        // per-printer block for it, so `resolve` rejects the pair, yet
        // `build_slice_profile` produces a profile from the hardware plus
        // the layer height alone.
        use sc_print_profiles::{SceneOverrides, resolve};

        let (printer, resin) = fixtures();
        assert!(
            resolve(&printer, &resin, &SceneOverrides::default()).is_err(),
            "the fixture pairing must be one a full resolve rejects",
        );

        let profile =
            build_slice_profile(&printer, &resin, None, AaLevel::Off, &cube_scene_top(0.97))
                .expect("a material without exposure calibration must still slice");
        assert_eq!(profile.build_volume().x_mm(), 153.36);
        match profile.layer_schedule() {
            LayerSchedule::Uniform(uniform) => assert_eq!(uniform.layer_height().value(), 0.05),
        }
    }
}
