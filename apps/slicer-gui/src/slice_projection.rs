/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Projecting the scene plus the chosen printer/resin and per-print
//! settings onto a slice profile.
//!
//! Two pieces: [`load_selected_profiles`] resolves the printer and
//! resin ids to full profiles on disk (the only I/O here), and
//! [`build_slice_profile`] is the pure projection: resolve the layer
//! height, read the model's height, derive the layer count, and project
//! the printer hardware onto a [`SliceProfile`]. The GUI slices the live
//! scene as
//! arranged; the only placement rule is that a scene with no geometry,
//! or one whose top is at or below the build plate (z = 0), cannot be
//! sliced.

use std::fmt;

use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::error::SceneError;
use sc_print_glue::{
    SliceInputFingerprintError, UnschedulableHeight, project_hardware_to_slice_profile,
    total_layers_for_height,
};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_print_profiles::{
    EffectiveMaterial, LoadMaterialProfileByIdError, LoadPrinterProfileByIdError, MaterialProfile,
    PrinterProfile, ResolveError, UserLibraryDirError, load_material_profile_by_id,
    load_printer_profile_by_id, resolve_layer_height, user_library_dir,
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

/// Load the full printer and resin profiles named by their ids, using
/// the same roots the catalog walks: the user library first, then the
/// bundled profiles.
pub(crate) fn load_selected_profiles(
    printer: &PrinterId,
    resin: &MaterialId,
) -> Result<(PrinterProfile, MaterialProfile), SliceFailure> {
    let user_lib = user_library_dir().map_err(SliceFailure::UserLibraryDir)?;
    let bundled = sc_app_paths::bundled_data_dir().join("profiles");
    let printer_profile = load_printer_profile_by_id(printer, None, &user_lib, &bundled)
        .map_err(SliceFailure::PrinterProfile)?;
    let resin_profile = load_material_profile_by_id(resin, None, &user_lib, &bundled)
        .map_err(SliceFailure::MaterialProfile)?;
    Ok((printer_profile, resin_profile))
}

/// The model's top Z in world millimetres, or a failure when the scene
/// has no geometry or sits at or below the build plate.
fn scene_top_z(scene: &Scene) -> Result<f64, SliceFailure> {
    let aabb = scene
        .aabb()
        .map_err(SliceFailure::SceneAabb)?
        .ok_or(SliceFailure::EmptySceneGeometry)?;
    let z_max = f64::from(aabb.max().coords()[2]);
    if z_max <= 0.0 {
        return Err(SliceFailure::SceneBelowPlate { z_max });
    }
    Ok(z_max)
}

/// Build a [`SliceProfile`] for the current scene. Resolves the layer
/// height (the only material parameter geometry slicing needs; the
/// exposure and motion parameters belong to the SLA output step), falls
/// back to it when no per-print first-layer height is set, derives the
/// layer count from the model's height, and projects the printer
/// hardware onto the engine's input port.
pub(crate) fn build_slice_profile(
    printer: &PrinterProfile,
    resin: &MaterialProfile,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    scene: &Scene,
) -> Result<SliceProfile, SliceFailure> {
    let material = EffectiveMaterial::from(resin.clone());
    let layer_height = resolve_layer_height(printer, &material).map_err(SliceFailure::Resolve)?;
    let first_layer_height = first_layer_height.unwrap_or(layer_height);
    let z_max = scene_top_z(scene)?;
    let total_layers =
        total_layers_for_height(z_max, layer_height.value(), first_layer_height.value())
            .map_err(SliceFailure::Projection)?;
    Ok(project_hardware_to_slice_profile(
        &printer.hardware,
        layer_height,
        aa_level,
        PackingMode::None,
        first_layer_height,
        total_layers,
    ))
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
mod scene_top_z_tests {
    use super::*;

    use std::sync::Arc;

    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::node::LeafPayload;
    use sc_mesh_core::transform::Transform3D;

    /// A cube spanning `min`..`max`, enough vertices for the AABB.
    fn cube_mesh(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
        let v = |x, y, z| Vertex::new([x, y, z]);
        let vertices = vec![
            v(min[0], min[1], min[2]),
            v(max[0], min[1], min[2]),
            v(max[0], max[1], min[2]),
            v(min[0], max[1], min[2]),
            v(min[0], min[1], max[2]),
            v(max[0], min[1], max[2]),
            v(max[0], max[1], max[2]),
            v(min[0], max[1], max[2]),
        ];
        let n = Normal::new([0.0, 0.0, 1.0]);
        let faces = vec![
            IndexedTriangle::from_raw_indices(n, [0, 1, 2]),
            IndexedTriangle::from_raw_indices(n, [0, 2, 3]),
        ];
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// One-cube scene under a single Group root.
    fn cube_scene(min: [f32; 3], max: [f32; 3]) -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group as root");
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(cube_mesh(min, max))),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        g
    }

    #[test]
    fn an_empty_scene_has_no_geometry() {
        let scene = Scene::new();
        assert!(matches!(
            scene_top_z(&scene),
            Err(SliceFailure::EmptySceneGeometry)
        ));
    }

    #[test]
    fn a_model_above_the_plate_reports_its_top_z() {
        let scene = cube_scene([-1.0, -1.0, 0.0], [1.0, 1.0, 0.97]);
        let z = scene_top_z(&scene).expect("a model above the plate has a top");
        assert!((z - 0.97).abs() < 1e-6, "got {z}");
    }

    #[test]
    fn a_model_at_or_below_the_plate_is_rejected() {
        let scene = cube_scene([-1.0, -1.0, -2.0], [1.0, 1.0, -1.0]);
        assert!(matches!(
            scene_top_z(&scene),
            Err(SliceFailure::SceneBelowPlate { .. })
        ));
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

    fn fixtures() -> (PrinterProfile, MaterialProfile) {
        (
            toml::from_str(PRINTER_TOML).expect("printer fixture parses"),
            toml::from_str(MATERIAL_TOML).expect("material fixture parses"),
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

    #[test]
    fn absent_first_layer_height_falls_back_to_the_material_layer_height() {
        let (printer, resin) = fixtures();
        // z_max 0.97, layer height 0.05, first layer 0.05 (fallback):
        // beyond = 0.92, ceil(0.92 / 0.05) = 19, total = 20.
        let profile =
            build_slice_profile(&printer, &resin, None, AaLevel::Off, &cube_scene_top(0.97))
                .expect("valid projection");
        match profile.layer_schedule() {
            LayerSchedule::Uniform(uniform) => {
                assert_eq!(uniform.layer_height().value(), 0.05);
                assert_eq!(uniform.first_layer_height().value(), 0.05);
                assert_eq!(uniform.total_layers().get(), 20);
            }
        }
    }

    #[test]
    fn an_explicit_first_layer_height_overrides_the_fallback() {
        let (printer, resin) = fixtures();
        // z_max 0.97, layer height 0.05, first layer 0.15:
        // beyond = 0.82, ceil(0.82 / 0.05) = 17, total = 18.
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
                assert_eq!(uniform.total_layers().get(), 18);
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
            resolve(
                &printer,
                &EffectiveMaterial::from(resin.clone()),
                &SceneOverrides::default(),
            )
            .is_err(),
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
