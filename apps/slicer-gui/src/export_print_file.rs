/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the Slice panel's "Export for printer"
//! action.
//!
//! The file is written from the held slice, never from what the panel
//! currently shows: the contours are the one input that cannot be
//! re-derived, so the printer, resin and per-print height they were
//! cut under are the ones the machine program is built from. Those
//! profiles live on disk and may have been edited since, so the
//! schedule is re-derived and compared against the held one before a
//! file whose header declares a layer count is written.
//!
//! Preparation and that comparison run on the UI thread; the per-layer
//! rasterisation runs on a worker while the export-progress panel is
//! up.

use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures_channel::oneshot;
use sc_print_glue::{
    UnprojectablePrint, creation_time_now, fallback_report_lines, project_to_print_program,
    slice_profile_for_selection,
};
use sc_print_profiles::{EffectiveMaterial, PrinterProfile, ResolveError, SceneOverrides, resolve};
use sc_scene::Scene;
use sc_sl1cer::VERSION;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::profile::PackingMode;
use sc_sla_formats::{PrintProgram, PrinterFileFormat, WriterError, write_printer_file};

use crate::MainWindow;
use crate::error_dialog::DialogManager;
use crate::export_progress::ExportPhase;
use crate::export_progress_panel::ExportProgressPanel;
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::scene_document::SceneDocument;
use crate::scene_export;
use crate::slice_panel::SlicePanelBridge;
use crate::slice_projection::{SliceFailure, load_selected_profiles};
use crate::slice_result::{ReSliceDecision, SliceParams, SliceResult, decide_re_slice};
use crate::viewport::ViewportController;

/// Why an export produced no file.
#[derive(Debug)]
pub(crate) enum ExportFailure {
    /// The current scene has not been sliced.
    NoSlice,
    /// The held slice no longer describes what the scene and the
    /// profiles on disk would now produce.
    Stale,
    /// The profiles the slice ran under could not be loaded, or no
    /// longer schedule the scene.
    Prepare(SliceFailure),
    /// The full print configuration does not resolve. Slicing needs
    /// the layer height alone; a printer file also needs the
    /// exposures, the waits and the motion.
    Resolve(ResolveError),
    /// A scheduled layer sits at a height the program cannot record.
    Program(UnprojectablePrint),
    /// The writer refused, or the file could not be written.
    Write(WriterError),
    /// The writing worker panicked. Carries the panic message.
    Panicked(String),
}

impl fmt::Display for ExportFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSlice => write!(
                f,
                "Nothing has been sliced yet, so there is nothing to write."
            ),
            Self::Stale => write!(
                f,
                "The held slice is out of date and no longer describes this plate. Slice it again before exporting.",
            ),
            Self::Prepare(err) => write!(f, "{err}"),
            Self::Resolve(err) => write!(f, "Could not resolve the print configuration: {err}"),
            Self::Program(err) => write!(f, "Cannot describe this print to the machine: {err}"),
            Self::Write(err) => write!(f, "Could not write the printer file: {err}"),
            Self::Panicked(msg) => write!(f, "The writer panicked: {msg}"),
        }
    }
}

/// A prepared export: the plate to rasterise, the program the machine
/// runs it under, the file both are written as, and the report of the
/// values in that program nobody calibrated.
#[derive(Debug)]
struct ExportJob {
    stack: ContourStack,
    program: PrintProgram,
    format: PrinterFileFormat,
    fallback_report: Vec<String>,
}

/// Whether the export action can run: a writable format is selected
/// and the held slice still matches the inputs the panel shows.
///
/// `current` is absent while no printer or resin is chosen. The
/// comparison is the one a Slice press makes, so anything that would
/// re-slice also blocks the export: the contours on hand would
/// otherwise be written under a program describing a different print.
pub(crate) fn export_button_enabled(
    held: Option<&SliceParams>,
    current: Option<&SliceParams>,
    geometry_changed: bool,
    format_selected: bool,
) -> bool {
    format_selected
        && current.is_some_and(|current| {
            decide_re_slice(held, current, geometry_changed) == ReSliceDecision::Unchanged
        })
}

/// Build the export from the held slice and the profiles it was cut
/// under, reading those profiles from the installed libraries.
fn prepare_export(
    document: &SceneDocument,
    format: PrinterFileFormat,
) -> Result<ExportJob, ExportFailure> {
    let held = document.held_slice().ok_or(ExportFailure::NoSlice)?;
    if document.slice_geometry_changed(held.params.geometry) {
        return Err(ExportFailure::Stale);
    }
    let scene = document
        .with_scene(|spatial| spatial.graph().clone())
        .ok_or(ExportFailure::NoSlice)?;
    let (printer, material) = load_selected_profiles(&held.params.printer, &held.params.resin)
        .map_err(ExportFailure::Prepare)?;
    export_job(held, &scene, &printer, &material, format)
}

/// Fuse a held slice with the profiles it was cut under into the job
/// the writers run.
///
/// The schedule is re-derived from `scene` and the profiles rather
/// than taken from the slice, and the two must agree: the program's
/// records pair with the contours by position, so a profile edited
/// since the slice ran would otherwise describe a print these layers
/// are not.
fn export_job(
    held: SliceResult,
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    format: PrinterFileFormat,
) -> Result<ExportJob, ExportFailure> {
    let config =
        resolve(printer, material, &SceneOverrides::default()).map_err(ExportFailure::Resolve)?;
    let profile = slice_profile_for_selection(
        scene,
        printer,
        material,
        None,
        held.params.first_layer_height,
        held.params.aa_level,
        PackingMode::None,
    )
    .map_err(|err| ExportFailure::Prepare(SliceFailure::from(err)))?;
    if profile.layer_schedule() != &held.schedule {
        return Err(ExportFailure::Stale);
    }
    let program = project_to_print_program(scene, &config, &profile, creation_time_now(), VERSION)
        .map_err(ExportFailure::Program)?;
    Ok(ExportJob {
        stack: held.merged,
        program,
        format,
        fallback_report: fallback_report_lines(&config),
    })
}

/// Wires the panel's export action. The callback holds the panel
/// bridge, which is where the selected printer file is known.
pub(crate) fn wire_export_print_file(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    panel: Rc<SlicePanelBridge>,
) {
    ui.on_slice_export_requested(move || {
        let Some(format) = panel.selected_print_format() else {
            return;
        };
        let document = ctrl.document();
        let dialogs = ctrl.dialogs();
        let export_progress = ctrl.export_progress();
        let default_stem = scene_export::default_export_stem(&document);
        if let Err(err) = slint::spawn_local(async move {
            run_export_flow(document, dialogs, export_progress, format, default_stem).await;
        }) {
            eprintln!("Failed to spawn printer-file export task: {err}");
        }
    });
}

/// Runs one export: prepare and check, ask for the target, then write
/// on a worker thread. Every refusal and failure surfaces its own
/// dialog.
async fn run_export_flow(
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    export_progress: Rc<ExportProgressPanel>,
    format: PrinterFileFormat,
    default_stem: Option<String>,
) {
    // Before the dialog: asking for a filename and refusing afterwards
    // is the ordering to avoid.
    let job = match prepare_export(&document, format) {
        Ok(job) => job,
        Err(err) => {
            show_export_failure(&dialogs, None, &err);
            return;
        }
    };
    // Also before the dialog: cancelling it is the last point at which
    // the user can act on what the warning says.
    for line in &job.fallback_report {
        eprintln!("{line}");
    }
    let Some(target) = pick_printer_file_target(default_stem, format).await else {
        return;
    };

    export_progress.begin();
    let sink = export_progress.sink();
    let (tx, rx) = oneshot::channel();
    let worker_target = target.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // The writer reports no progress of its own, so the panel
            // names the phase and nothing finer.
            sink.set(ExportPhase::Write, 0, 1);
            let written = write_printer_file(&job.stack, &worker_target, job.format, &job.program)
                .map_err(ExportFailure::Write);
            sink.set(ExportPhase::Write, 1, 1);
            written
        }))
        .unwrap_or_else(|payload| Err(ExportFailure::Panicked(panic_message(&payload))));
        // Deliver on the UI thread so the awaiting task wakes there.
        let _ = slint::invoke_from_event_loop(move || {
            let _ = tx.send(result);
        });
    });

    let outcome = rx.await;
    export_progress.end();
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => show_export_failure(&dialogs, Some(&target), &err),
        // The sender dropped without sending (event loop gone at
        // shutdown): there is nothing left to report to.
        Err(_canceled) => {}
    }
}

fn show_export_failure(dialogs: &Rc<DialogManager>, target: Option<&Path>, err: &ExportFailure) {
    dialogs.show_error(
        ctr!("ErrorDialog", "Cannot export for the printer").as_str(),
        target.map(|path| ctr!("ErrorDialog", "File: {}", path.display()).to_string()),
        err.to_string(),
        String::new(),
    );
}

/// Asks for the file to write, seeded with the format's own extension.
async fn pick_printer_file_target(
    default_stem: Option<String>,
    format: PrinterFileFormat,
) -> Option<PathBuf> {
    let extension = format.extension();
    let mut dialog = rfd::AsyncFileDialog::new().add_filter(
        ctr!("FileDialog", "Printer files").to_string(),
        &[extension],
    );
    if let Some(stem) = default_stem {
        dialog = dialog.set_file_name(format!("{stem}.{extension}"));
    }
    dialog.save_file().await.map(|h| h.path().to_path_buf())
}

#[cfg(test)]
mod export_button_enabled_tests {
    use sc_print_primitives::{AaLevel, MaterialId, PrinterId};

    use super::*;
    use crate::scene_document::SceneRevision;

    fn params() -> SliceParams {
        SliceParams {
            printer: PrinterId::new("elegoo-mars-5-goo").expect("valid id"),
            resin: MaterialId::new("standard-405").expect("valid id"),
            first_layer_height: None,
            aa_level: AaLevel::Off,
            geometry: SceneRevision::from_top_edit_id(Some(3)),
        }
    }

    #[test]
    fn a_held_slice_matching_the_selection_can_be_exported() {
        assert!(export_button_enabled(
            Some(&params()),
            Some(&params()),
            false,
            true,
        ));
    }

    #[test]
    fn a_printer_declaring_no_writable_format_blocks_the_export() {
        assert!(!export_button_enabled(
            Some(&params()),
            Some(&params()),
            false,
            false,
        ));
    }

    #[test]
    fn an_unsliced_scene_has_nothing_to_export() {
        assert!(!export_button_enabled(None, Some(&params()), false, true));
    }

    #[test]
    fn without_a_printer_and_a_resin_there_is_nothing_to_compare_against() {
        assert!(!export_button_enabled(Some(&params()), None, false, true));
    }

    #[test]
    fn a_resin_picked_since_the_slice_blocks_the_export() {
        let current = SliceParams {
            resin: MaterialId::new("abs-like-405").expect("valid id"),
            ..params()
        };
        assert!(!export_button_enabled(
            Some(&params()),
            Some(&current),
            false,
            true,
        ));
    }

    #[test]
    fn a_scene_edited_since_the_slice_blocks_the_export() {
        assert!(!export_button_enabled(
            Some(&params()),
            Some(&params()),
            true,
            true,
        ));
    }
}

#[cfg(test)]
mod export_job_tests {
    use std::sync::Arc;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
    use sc_print_profiles::MaterialProfile;
    use sc_scene::LeafPayload;
    use sc_sl1cer::contour::{ContourLayer, ContourStack};
    use sc_sl1cer::profile::LayerSchedule;
    use sc_sla_formats::GooVersion;

    use super::*;
    use crate::scene_document::SceneRevision;

    const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-mars-5-goo"
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

[per_printer."elegoo-mars-5-goo"]
normal_exposure_sec = 2.5
bottom_exposure_sec = 30.0
bottom_layer_count = 8
lift_distance_mm = 5.0
lift_speed_mm_min = 120.0
retract_speed_mm_min = 150.0
"#;

    /// The same resin with its calibration for this printer removed,
    /// which is what every shipped resin looks like on a machine it
    /// was never measured on.
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

    const GOO: PrinterFileFormat = PrinterFileFormat::Goo(GooVersion::V3_0);

    fn profiles(material_toml: &str) -> (PrinterProfile, EffectiveMaterial) {
        let printer: PrinterProfile = toml::from_str(PRINTER_TOML).expect("printer profile");
        let material: MaterialProfile = toml::from_str(material_toml).expect("material profile");
        (printer, EffectiveMaterial::from(material))
    }

    /// One cube leaf as the scene's own root, spanning z = -0.5 to
    /// 0.5, so half of it stands above the plate.
    fn cube_scene() -> Scene {
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
        scene
    }

    /// The schedule this scene and these profiles derive, which is
    /// what a slice of them would have run under.
    fn derived_schedule(
        scene: &Scene,
        printer: &PrinterProfile,
        material: &EffectiveMaterial,
    ) -> LayerSchedule {
        *slice_profile_for_selection(
            scene,
            printer,
            material,
            None,
            None,
            AaLevel::Off,
            PackingMode::None,
        )
        .expect("the cube schedules")
        .layer_schedule()
    }

    fn held_slice(schedule: LayerSchedule) -> SliceResult {
        let layers: Vec<ContourLayer> = (0..schedule.total_layers())
            .map(|_| ContourLayer::new(0.05, vec![]))
            .collect();
        SliceResult {
            merged: ContourStack::new_unchecked(layers),
            schedule,
            token: String::new(),
            params: SliceParams {
                printer: PrinterId::new("elegoo-mars-5-goo").expect("valid id"),
                resin: MaterialId::new("sirayatech-blu").expect("valid id"),
                first_layer_height: None,
                aa_level: AaLevel::Off,
                geometry: SceneRevision::from_top_edit_id(None),
            },
        }
    }

    #[test]
    fn the_program_carries_one_record_per_held_layer() {
        let (printer, material) = profiles(MATERIAL_TOML);
        let scene = cube_scene();
        let held = held_slice(derived_schedule(&scene, &printer, &material));

        let job = export_job(held, &scene, &printer, &material, GOO).expect("the export prepares");

        assert_eq!(job.program.layers.len(), job.stack.layers().len());
        assert_eq!(job.format, GOO);
    }

    #[test]
    fn the_program_names_the_machine_and_the_resin_it_was_sliced_for() {
        let (printer, material) = profiles(MATERIAL_TOML);
        let scene = cube_scene();
        let held = held_slice(derived_schedule(&scene, &printer, &material));

        let job = export_job(held, &scene, &printer, &material, GOO).expect("the export prepares");

        assert_eq!(job.program.machine_name.as_str(), "Mars 5");
        assert_eq!(job.program.material_name.as_str(), "Blu");
    }

    #[test]
    fn the_job_reports_the_values_the_resin_left_to_the_resolver() {
        // The fixture resin names what resolution requires and none of
        // the optional values, so the whole optional set falls back.
        let (printer, material) = profiles(MATERIAL_TOML);
        let scene = cube_scene();
        let held = held_slice(derived_schedule(&scene, &printer, &material));

        let job = export_job(held, &scene, &printer, &material, GOO).expect("the export prepares");

        assert_eq!(
            job.fallback_report.first().map(String::as_str),
            Some("warning: 11 print values are not calibrated for this printer:"),
        );
        assert_eq!(job.fallback_report.len(), 12, "{:?}", job.fallback_report);
    }

    #[test]
    fn a_slice_whose_schedule_no_longer_matches_is_refused() {
        // A layer height the profiles no longer produce stands in for
        // a profile edited between the slice and the export.
        let (printer, material) = profiles(MATERIAL_TOML);
        let scene = cube_scene();
        let stale = LayerSchedule::uniform(0.1, 0.1, 5).expect("a positive, non-empty schedule");

        let err = export_job(held_slice(stale), &scene, &printer, &material, GOO)
            .expect_err("a mismatched schedule cannot be written");

        assert!(matches!(err, ExportFailure::Stale), "{err}");
    }

    #[test]
    fn a_resin_without_print_calibration_cannot_be_written() {
        // The same pairing slices geometry: a printer file also needs
        // the exposures, which this resin carries for no printer.
        let (printer, material) = profiles(UNCALIBRATED_MATERIAL_TOML);
        let scene = cube_scene();
        let held = held_slice(derived_schedule(&scene, &printer, &material));

        let err = export_job(held, &scene, &printer, &material, GOO)
            .expect_err("an unresolvable configuration cannot be written");

        assert!(matches!(err, ExportFailure::Resolve(_)), "{err}");
    }
}

#[cfg(test)]
mod prepare_export_tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
    use sc_scene::LeafPayload;
    use sc_sl1cer::contour::{ContourLayer, ContourStack};
    use sc_sl1cer::profile::LayerSchedule;
    use sc_sla_formats::GooVersion;

    use super::*;
    use crate::scene_document::SceneEdit;

    const GOO: PrinterFileFormat = PrinterFileFormat::Goo(GooVersion::V3_0);

    fn cube_document() -> Rc<SceneDocument> {
        let mut scene = Scene::new();
        let root = scene.add_group(None, Transform3D::Identity);
        scene.add_root(root).expect("group root");
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_child(root, leaf).expect("attach leaf");

        let document = SceneDocument::new();
        document.install_graph(scene, PathBuf::from("/tmp/export_print_file_fixture.stl"));
        document
    }

    fn hold_a_slice(document: &SceneDocument) {
        document.set_slice_result(SliceResult {
            merged: ContourStack::new_unchecked(vec![ContourLayer::new(0.05, vec![])]),
            schedule: LayerSchedule::uniform(0.05, 0.05, 1).expect("a one-layer schedule"),
            token: String::new(),
            params: SliceParams {
                printer: PrinterId::new("elegoo-mars-5-goo").expect("valid id"),
                resin: MaterialId::new("sirayatech-blu").expect("valid id"),
                first_layer_height: None,
                aa_level: AaLevel::Off,
                geometry: document.scene_revision(),
            },
        });
    }

    #[test]
    fn an_unsliced_document_has_nothing_to_export() {
        let document = cube_document();

        let err = prepare_export(&document, GOO).expect_err("nothing is held");

        assert!(matches!(err, ExportFailure::NoSlice), "{err}");
    }

    #[test]
    fn a_scene_edited_since_the_slice_is_refused_before_any_profile_is_read() {
        let document = cube_document();
        hold_a_slice(&document);
        let node_id = document
            .with_scene(|spatial| spatial.leaf_id_at_index(0))
            .flatten()
            .expect("the fixture has one leaf");
        let edit = SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        document.apply_edit(edit.clone()).expect("the edit applies");
        document.push_edit(edit);

        let err = prepare_export(&document, GOO).expect_err("the plate moved under the slice");

        assert!(matches!(err, ExportFailure::Stale), "{err}");
    }
}
