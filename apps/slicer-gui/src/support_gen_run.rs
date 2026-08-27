/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for Tools > "Generate Support":
//! the GUI's integration touchpoint for `sc-geom`, mirroring
//! `apps/slicer-cli`'s `support-gen` subcommand. Not a real
//! support-generation feature.
//!
//! Unlike `SliceRun` / `RepairRun`, no `std::thread::spawn` happens
//! here: `sc_geom::gen_support` already runs the algorithm on its own
//! worker thread and hands back a pollable `JobHandle`, so this driver
//! just holds that handle and polls `JobHandle::progress()` /
//! `try_poll()` from a `slint::Timer` on the UI thread, the same
//! cadence `SliceRun` polls its atomics at. `artificial_delay_ms` is
//! set deliberately high here (see `sc_geom::TrivialSupportSettings`'s
//! own doc comment) purely so that progress bar has ~2 real seconds
//! to animate instead of resolving instantly.
//!
//! The support-generation job's "boundary", if it is to be the printer's real build volume.
//! `ViewportController::build_volume()`, the same envelope the
//! viewport draws as the plate/cage and the Slice panel derives from
//! the selected printer: read fresh on every press so a printer
//! change since the window opened is picked up. That envelope's own
//! convention already puts the bed at `Z = 0`, so a strut cast
//! downward from the object lands exactly on the bed without this
//! driver needing to know that itself.
//!
//! On completion each generated support structure mesh component is pasted into the scene
//! through the same `SceneFragment` + `SceneDocument::paste_fragment`
//! primitive Paste uses, so it is a real, "undo"able scene object (shows
//! up in the object list, survives save/export) rather than a
//! throwaway preview. Then the run is summarised in an info dialog alongside it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use sc_geom::{
    BoundaryShape, JobError, JobHandle, JobRecipe, PrintPlate, ResolutionRecord,
    SupportStructureResult, SupportStructureSettings, TrivialSupport, TrivialSupportInferredSpecs,
    TrivialSupportSettings, gen_support,
};
use sc_mesh_core::Vector3D;
use sc_mesh_core::transform::Transform3D;
use sc_scene_edit::SceneFragment;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::build_volume::BuildVolume;
use crate::error_dialog::DialogManager;
use crate::i18n::ctr;
use crate::scene_document::SceneDocument;
use crate::viewport::ViewportController;

/// Cadence of the UI-side progress poll while a support generation job is in
/// flight. Same as `SliceRun`'s.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

type GenSupportHandle =
    JobHandle<SupportStructureResult<TrivialSupportInferredSpecs>, sc_geom::SupportPartial>;

pub(crate) struct GenSupportRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    // Read live at each press for its current `build_volume()` --
    // never cached, so a printer change picks up without reopening
    // the window.
    controller: Rc<ViewportController>,
    // The in-flight job, if any. Polled by `poll_timer`; `None` when
    // no run is in progress.
    handle: RefCell<Option<GenSupportHandle>>,
    // Poll timer driving the progress panel. Behind an `Rc` so its own
    // callback can stop it through a `Weak` without a cycle.
    poll_timer: Rc<slint::Timer>,
}

impl GenSupportRun {
    /// Wires the Tools menu callback and returns the driver, which the
    /// caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        controller: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            controller,
            handle: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
        });
        {
            let run = run.clone();
            ui.on_support_gen_requested(move || run.start());
        }
        run
    }

    /// Handles the menu press: gate, clone, submit, start polling. The
    /// menu item's `enabled` binding gates most of this too; the
    /// checks keep the path safe against a racing double-activation.
    fn start(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading() || self.handle.borrow().is_some() {
            return;
        }
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        let boundary_shape = boundary_box_from_build_volume(self.controller.build_volume());
        let plate = PrintPlate {
            boundary: boundary_shape.clone(),
            plate_normal: Vector3D::new([0.0, 0.0, 1.0]),
        };
        let settings = SupportStructureSettings {
            gravity: Vector3D::new([0.0, 0.0, -1.0]),
            boundary_shape,
            explicit_locations: Vec::new(), // auto-detect
            allow_expand_touch: false,
            allow_expand_trunk: false,
        };
        let algorithm_settings = TrivialSupportSettings {
            ..TrivialSupportSettings::default()
        };

        match gen_support(
            &TrivialSupport,
            &scene,
            &plate,
            &settings,
            &algorithm_settings,
        ) {
            Ok(handle) => {
                ui.set_support_gen_in_progress(true);
                ui.set_support_gen_progress(0.0);
                ui.set_support_gen_phase(slint::SharedString::default());
                *self.handle.borrow_mut() = Some(handle);
                self.start_poll();
            }
            Err(err) => {
                self.dialogs.show_error(
                    &ctr!("GenSupportDialog", "Support generation failed"),
                    None,
                    err.to_string(),
                    String::new(),
                );
            }
        }
    }

    /// Starts (or restarts) the progress poll. Stops itself once the
    /// handle resolves or the window disappears.
    fn start_poll(self: &Rc<Self>) {
        let run = self.clone();
        let timer_weak = Rc::downgrade(&self.poll_timer);
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let Some(ui) = run.ui.upgrade() else {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                };

                let finished = {
                    let slot = run.handle.borrow();
                    let Some(handle) = slot.as_ref() else {
                        if let Some(timer) = timer_weak.upgrade() {
                            timer.stop();
                        }
                        return;
                    };
                    let progress = handle.progress();
                    ui.set_support_gen_progress(progress.fraction());
                    ui.set_support_gen_phase(progress.phase.into());
                    handle.try_poll()
                };

                if let Some(outcome) = finished {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    run.handle.borrow_mut().take();
                    ui.set_support_gen_in_progress(false);
                    run.finish(outcome);
                }
            },
        );
    }

    /// Pastes every generated support structure mesh component into the scene as a real, undoable
    /// object and reports the run through the info/error dialog.
    fn finish(
        &self,
        outcome: Result<SupportStructureResult<TrivialSupportInferredSpecs>, JobError>,
    ) {
        match outcome {
            Ok(result) => {
                let total = result.meshes.len();
                let mut inserted = 0usize;
                let mut rejection = None;
                for mesh in result.meshes {
                    let fragment =
                        SceneFragment::from_leaf_snapshot(Arc::new(mesh), Transform3D::Identity);
                    match self.document.paste_fragment(&fragment, [0.0, 0.0, 0.0]) {
                        Ok(Some(_)) => inserted += 1,
                        Ok(None) => {}
                        Err(error) => {
                            rejection.get_or_insert(error);
                        }
                    };
                }
                if let Some(error) = rejection {
                    self.dialogs.show_error(
                        &ctr!("GenSupportDialog", "Support-structures generation failed"),
                        None,
                        ctr!(
                            "GenSupportDialog",
                            "The scene rejected one or more generated support structures; \
                             nothing was changed for those."
                        )
                        .to_string(),
                        error.to_string(),
                    );
                    return;
                }
                let material_g = *result.algorithm_specs.estimated_material_g.value();
                self.dialogs.show_info(
                    &ctr!("GenSupportDialog", "Support-structures generated"),
                    summary_message(inserted, total, material_g),
                    details_message(&result.recipe, &result.resolution_trail),
                );
            }
            Err(err) => {
                self.dialogs.show_error(
                    &ctr!("GenSupportDialog", "Support-structures generation failed"),
                    None,
                    err.to_string(),
                    String::new(),
                );
            }
        }
    }
}

/// The printer's real build envelope as a `sc_geom` boundary box.
/// `BuildVolume`'s own convention already puts the bed at `Z = 0` and
/// the chamber ceiling at `Z = height_mm`, so a strut cast downward
/// from the object lands exactly on the bed.
/// It is for situations where 3D-printer's "bed Z" is inferred, rather than explicitly given.
fn boundary_box_from_build_volume(build_volume: BuildVolume) -> BoundaryShape {
    let min = build_volume.envelope_min_mm();
    let max = build_volume.envelope_max_mm();
    BoundaryShape::BuildVolumeBox {
        min: Vector3D::new([min.x as f64, min.y as f64, min.z as f64]),
        max: Vector3D::new([max.x as f64, max.y as f64, max.z as f64]),
    }
}

/// One-line rollup for the dialog's message.
fn summary_message(inserted: usize, total: usize, material_g: f64) -> String {
    if inserted < total {
        return ctr!(
            "GenSupportDialog",
            "{} of {} Support structure(s) added to the scene. Estimated material: {} g.",
            inserted,
            total,
            format!("{material_g:.2}")
        )
        .to_string();
    }
    ctr!(
        "GenSupportDialog",
        "{} Support structure(s) added to the scene. Estimated material: {} g.",
        inserted,
        format!("{material_g:.2}")
    )
    .to_string()
}

/// Recipe + resolution-trail detail lines, for the dialog's collapsed
/// "Show details" section.
fn details_message(recipe: &JobRecipe, resolution_trail: &[ResolutionRecord]) -> String {
    let mut lines = vec![format!(
        "algorithm={} version={}",
        recipe.algorithm_id, recipe.algorithm_version
    )];
    for record in resolution_trail {
        lines.push(format!(
            "{} <- {:?} = {}",
            record.field, record.source, record.final_value
        ));
    }
    lines.join("\n")
}
