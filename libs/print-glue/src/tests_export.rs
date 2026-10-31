/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::error::Error;

use sc_mesh_core::primitives::IndexedTriangle;
use sc_mesh_fixtures::unit_cube_mesh;
use sc_print_primitives::{AaLevel, PositiveFiniteMm};
use sc_print_profiles::{
    EffectiveMaterial, MaterialProfile, PrinterProfile, ResolveError, ResolvedField,
    SceneOverrides, resolve_layer_height,
};
use sc_scene::Scene;
use sc_scene_fixtures::{meshes_as_roots, trio_as_roots};
use sc_sl1cer::CancelToken;
use sc_sl1cer::contour::{ContourLayer, ContourStack};
use sc_sl1cer::profile::LayerSchedule;
use sc_sla_formats::RasterSetup;

use super::export::{ExportJob, UnexportableSlice, prepare_export};
use super::fallback_report::fallbacks_used;
use super::print_program::{SlabTopOutOfRange, UnprojectablePrint};
use super::scale_compensation::SupportOwners;
use super::slice_plate::{PlateContours, SliceRun, SlicedPlate, SlicingParams, slice_plate};

const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-mars-5"
manufacturer = "ELEGOO"
family = "Mars"
name = "Mars 5"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 143.43, y_mm = 89.6, z_mm = 150.0 }
pixel_resolution = { x = 4098, y = 2560 }
pixel_size_microns = { x = 35.0, y = 35.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "goo", version = "v3.0" }]
supports_grayscale = true
two_stage_lift = false
transition_layers = 5
has_camera = false

[constraints]
z_step_mm = 0.01
light_intensity_mw_per_cm2 = 5.0
"#;

const MATERIAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }

[per_printer."elegoo-mars-5"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

/// The same resin with its calibration for this printer removed, which is what
/// every shipped resin looks like on a machine it was never measured on.
const UNCALIBRATED_MATERIAL_TOML: &str = r#"
[identity]
id = "sirayatech-blu"
brand = "Siraya Tech"
family = "standard"
name = "Blu"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
scale_compensation_pct = { x = 0.0, y = 0.0, z = 0.0 }
"#;

const SLICER_VERSION: &str = "0.0.12";

fn printer() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_TOML).expect("printer fixture parses")
}

fn material(toml: &str) -> EffectiveMaterial {
    let profile: MaterialProfile = toml_edit::de::from_str(toml).expect("material fixture parses");
    EffectiveMaterial::from(profile)
}

fn calibrated() -> EffectiveMaterial {
    material(MATERIAL_TOML)
}

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).expect("test lengths are positive finite")
}

fn resin_defaults() -> SlicingParams {
    SlicingParams {
        layer_height_request: None,
        first_layer_height: None,
        aa_level: AaLevel::Off,
    }
}

/// Requests 0.25 mm layers, which the 0.01 mm Z step leaves as they are.
fn quarter_mm_layers() -> SlicingParams {
    SlicingParams {
        layer_height_request: Some(mm(0.25)),
        ..resin_defaults()
    }
}

fn slice(scene: &Scene, material: &EffectiveMaterial, params: &SlicingParams) -> SlicedPlate {
    match slice_plate(
        scene,
        &printer(),
        material,
        params,
        &SupportOwners::new(),
        &CancelToken::new(),
        |_| {},
    )
    .expect("the fixture scene slices")
    {
        SliceRun::Completed(plate) => plate,
        SliceRun::Cancelled => panic!("an uncancelled run cannot report Cancelled"),
    }
}

fn contours_of(scene: &Scene, params: &SlicingParams) -> PlateContours {
    slice(scene, &calibrated(), params)
        .merge()
        .expect("the leaves of 1 run merge")
}

fn export(
    scene: &Scene,
    contours: PlateContours,
    material: &EffectiveMaterial,
    overrides: &SceneOverrides,
) -> Result<ExportJob, UnexportableSlice> {
    prepare_export(
        scene,
        contours,
        &printer(),
        material,
        overrides,
        AaLevel::Off,
        SLICER_VERSION,
    )
}

fn requesting(layer_height: f64) -> SceneOverrides {
    SceneOverrides {
        layer_height_mm: Some(mm(layer_height)),
        ..SceneOverrides::default()
    }
}

#[test]
fn contours_cut_at_a_requested_layer_height_export_under_the_same_request() {
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &quarter_mm_layers());

    let job =
        export(&scene, contours, &calibrated(), &requesting(0.25)).expect("the export prepares");

    assert_eq!(job.program.layer_height_mm, mm(0.25));
    // The prisms reach z = 2: 8 slabs of 0.25 mm.
    let slab_tops: Vec<f64> = job
        .program
        .layers
        .iter()
        .map(|layer| layer.slab_top_mm.value())
        .collect();
    assert_eq!(slab_tops, [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0]);
}

#[test]
fn the_same_contours_exported_without_the_request_are_refused() {
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &quarter_mm_layers());

    let err = export(&scene, contours, &calibrated(), &SceneOverrides::default())
        .expect_err("the resin's own layer height is 0.05 mm");

    assert_eq!(
        err,
        UnexportableSlice::LayerHeightMismatch {
            sliced: mm(0.25),
            resolved: mm(0.05),
        },
    );
}

#[test]
fn a_first_layer_off_the_z_step_is_refused() {
    // Slicing snaps the first layer onto the Z step, so only contours built by
    // hand reach this check.
    let layer_height = resolve_layer_height(&printer(), &calibrated(), None)
        .expect("the resin names a layer height");
    let schedule = LayerSchedule::uniform(layer_height.value(), 0.057, 3)
        .expect("a positive, non-empty schedule");
    let stack = ContourStack::new(
        schedule
            .iter_z()
            .map(|z| ContourLayer::new(z, vec![]))
            .collect(),
    )
    .expect("z is strictly increasing");
    let contours = PlateContours::new(stack, schedule).expect("3 layers pair with 3");

    let err = export(
        &trio_as_roots(),
        contours,
        &calibrated(),
        &SceneOverrides::default(),
    )
    .expect_err("a 0.057 mm first layer is off a 0.01 mm step");

    match err {
        UnexportableSlice::FirstLayerOffZStep {
            first_layer,
            nearest,
        } => {
            assert_eq!(first_layer, mm(0.057));
            assert!((nearest.value() - 0.06).abs() < 1e-12, "nearest: {nearest}");
        }
        other => panic!("expected a first layer off the Z step, got {other:?}"),
    }
}

#[test]
fn a_first_layer_that_slicing_snapped_onto_the_z_step_is_accepted() {
    let scene = trio_as_roots();
    let params = SlicingParams {
        first_layer_height: Some(mm(0.057)),
        ..resin_defaults()
    };
    let contours = contours_of(&scene, &params);

    let job = export(&scene, contours, &calibrated(), &SceneOverrides::default())
        .expect("the export prepares");

    let first = job.program.layers[0].slab_top_mm.value();
    assert!((first - 0.06).abs() < 1e-12, "first slab top: {first}");
}

#[test]
fn a_fresh_slice_exports_with_the_raster_and_the_layers_it_was_sliced_with() {
    let scene = trio_as_roots();
    let params = SlicingParams {
        aa_level: AaLevel::X4,
        ..resin_defaults()
    };
    let plate = slice(&scene, &calibrated(), &params);
    let sliced_raster = RasterSetup::from_profile(plate.profile());
    let sliced_tops: Vec<f64> = plate.schedule().iter_slab_tops().collect();
    let contours = plate.merge().expect("the leaves of 1 run merge");

    let job = prepare_export(
        &scene,
        contours,
        &printer(),
        &calibrated(),
        &SceneOverrides::default(),
        AaLevel::X4,
        SLICER_VERSION,
    )
    .expect("the export prepares");

    assert_eq!(job.program.raster, sliced_raster);
    let tops: Vec<f64> = job
        .program
        .layers
        .iter()
        .map(|layer| layer.slab_top_mm.value())
        .collect();
    assert_eq!(tops, sliced_tops);
}

#[test]
fn the_program_carries_a_record_for_each_contour_layer() {
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &resin_defaults());

    let job = export(&scene, contours, &calibrated(), &SceneOverrides::default())
        .expect("the export prepares");

    assert_eq!(
        job.program.layers.len(),
        job.contours.stack().layers().len()
    );
}

#[test]
fn the_program_names_the_machine_and_the_resin_it_was_sliced_for() {
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &resin_defaults());

    let job = export(&scene, contours, &calibrated(), &SceneOverrides::default())
        .expect("the export prepares");

    assert_eq!(job.program.machine_name.as_str(), "Mars 5");
    assert_eq!(job.program.material_name.as_str(), "Blu");
}

#[test]
fn the_job_reports_the_values_the_resin_left_to_the_resolver() {
    // The fixture resin names what resolution requires and none of the optional
    // values, so the whole optional set falls back.
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &resin_defaults());

    let job = export(&scene, contours, &calibrated(), &SceneOverrides::default())
        .expect("the export prepares");

    assert_eq!(fallbacks_used(job.config()).len(), 12);
}

#[test]
fn a_resin_without_print_calibration_is_refused_by_the_resolver() {
    // The same pairing slices geometry: a printer file also needs the
    // exposures, which this resin carries for no printer.
    let scene = trio_as_roots();
    let uncalibrated = material(UNCALIBRATED_MATERIAL_TOML);
    let contours = slice(&scene, &uncalibrated, &resin_defaults())
        .merge()
        .expect("the leaves of 1 run merge");

    let err = export(&scene, contours, &uncalibrated, &SceneOverrides::default())
        .expect_err("an unresolvable configuration cannot be written");

    assert!(matches!(err, UnexportableSlice::Configuration(_)), "{err}");
}

#[test]
fn a_scene_that_cannot_be_measured_is_refused_by_the_program() {
    // Only the program's volume reads the scene, so contours cut from a sound
    // scene reach that measure.
    let contours = contours_of(&trio_as_roots(), &resin_defaults());
    let mut broken = unit_cube_mesh();
    broken.faces[0] = IndexedTriangle::from_raw_indices([0, 1, 99]);

    let err = export(
        &meshes_as_roots([broken]),
        contours,
        &calibrated(),
        &SceneOverrides::default(),
    )
    .expect_err("a scene with a dangling vertex index has no volume");

    assert!(
        matches!(
            err,
            UnexportableSlice::Program(UnprojectablePrint::Scene(_))
        ),
        "{err}"
    );
}

#[test]
fn a_configuration_failure_keeps_the_resolver_message() {
    let cause = ResolveError::MissingRequiredField {
        field: ResolvedField::NormalExposureSec,
    };
    let err = UnexportableSlice::Configuration(cause);

    assert_eq!(
        err.to_string(),
        format!("could not resolve the print configuration: {cause}"),
    );
    assert_eq!(
        err.source().map(ToString::to_string),
        Some(cause.to_string())
    );
}

#[test]
fn a_layer_height_mismatch_names_both_heights() {
    let err = UnexportableSlice::LayerHeightMismatch {
        sliced: mm(0.25),
        resolved: mm(0.05),
    };

    assert_eq!(
        err.to_string(),
        "the print configuration resolves 0.05 mm layers for contours cut at 0.25 mm",
    );
    assert!(err.source().is_none());
}

#[test]
fn a_first_layer_off_the_z_step_names_the_nearest_height_on_it() {
    let err = UnexportableSlice::FirstLayerOffZStep {
        first_layer: mm(0.057),
        nearest: mm(0.06),
    };

    assert_eq!(
        err.to_string(),
        "the first layer is 0.057 mm high, off the printer's Z step; the nearest height on the step is 0.06 mm",
    );
    assert!(err.source().is_none());
}

#[test]
fn a_program_failure_keeps_the_projection_message() {
    let cause = UnprojectablePrint::SlabTop(SlabTopOutOfRange {
        layer_index: 1,
        z_mm: f64::INFINITY,
    });
    let err = UnexportableSlice::Program(cause);

    assert_eq!(
        err.to_string(),
        "cannot describe this print to the machine: layer 1 sits at z = inf mm, which is not a positive finite height",
    );
    assert!(err.source().is_some());
}

/// `contours` with `extra` empty layers stacked on top, on the same grid.
fn with_dark_top(contours: PlateContours, extra: u32) -> PlateContours {
    let LayerSchedule::Uniform(grid) = *contours.schedule();
    let schedule = LayerSchedule::uniform(
        grid.layer_height().value(),
        grid.first_layer_height().value(),
        grid.total_layers().get() + extra,
    )
    .expect("a positive, non-empty schedule");
    let mut layers = contours.stack().layers().to_vec();
    layers.extend(
        schedule
            .iter_z()
            .skip(layers.len())
            .map(|z| ContourLayer::new(z, vec![])),
    );
    let stack = ContourStack::new(layers).expect("z is strictly increasing");
    PlateContours::new(stack, schedule).expect("the counts pair")
}

#[test]
fn empty_layers_on_top_are_left_out_of_the_printer_file() {
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &resin_defaults());
    let lit = contours.stack().layers().len();
    let lit_tops: Vec<f64> = contours.schedule().iter_slab_tops().collect();

    let job = export(
        &scene,
        with_dark_top(contours, 2),
        &calibrated(),
        &SceneOverrides::default(),
    )
    .expect("the export prepares");

    assert_eq!(job.contours.stack().layers().len(), lit);
    assert_eq!(job.contours.schedule().total_layers() as usize, lit);
    let tops: Vec<f64> = job
        .program
        .layers
        .iter()
        .map(|layer| layer.slab_top_mm.value())
        .collect();
    assert_eq!(tops, lit_tops);
}

#[test]
fn a_slice_whose_top_layer_is_lit_exports_every_layer() {
    let scene = trio_as_roots();
    let contours = contours_of(&scene, &resin_defaults());
    let layers = contours.stack().layers().len();

    let job = export(&scene, contours, &calibrated(), &SceneOverrides::default())
        .expect("the export prepares");

    assert_eq!(job.program.layers.len(), layers);
}

#[test]
fn a_plate_that_lights_nothing_keeps_its_layers() {
    let schedule = LayerSchedule::uniform(0.05, 0.05, 3).expect("a positive, non-empty schedule");
    let stack = ContourStack::new(
        schedule
            .iter_z()
            .map(|z| ContourLayer::new(z, vec![]))
            .collect(),
    )
    .expect("z is strictly increasing");
    let contours = PlateContours::new(stack, schedule).expect("3 layers pair with 3");

    let job = export(
        &trio_as_roots(),
        contours,
        &calibrated(),
        &SceneOverrides::default(),
    )
    .expect("the export prepares");

    assert_eq!(job.program.layers.len(), 3);
}
