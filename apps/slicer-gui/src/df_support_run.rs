/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for Tools > "Generate Supports (DragonFruit)" (SC-79):
//! runs `sc_geom::df_supports` (the native port of DragonFruit's
//! auto-support: island scan, placement, bracing, raft) on the scene.
//!
//! Like `GenSupportRun`, the job runs on its own worker thread behind a
//! `JobHandle`, and a `slint::Timer` polls its progress into the shared
//! support-generation panel, which shows a Cancel button while this run is in
//! flight. On completion the supports and raft, already tessellated and
//! welded into one mesh, are pasted into the scene through
//! `SceneDocument::paste_fragment`: one undoable object, saved and exported
//! like any other. A later run leaves that object out of its models.
//!
//! **Background prescan (SC-82/SC-83).** The island and overhang scans depend
//! only on the geometry, its pose and the scan settings, so while the scene
//! sits still a second, lower-key job runs them ahead of time
//! (`start_df_prescan`, in a pool of half the cores) and the next run starts
//! from them. Nothing of it is on the transform path: a scene event only
//! stamps the time; a poll starts the prescan once the scene has been still
//! for [`PRESCAN_SETTLE`], and cancels it when the scene moves on. Reuse is
//! safe by construction: the core keys the scans on the baked world geometry,
//! so a stale scan never matches. Off with the Preferences > Slicing switch.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_geom::JobError;
use sc_geom::JobHandle;
use sc_geom::df_supports::{
    DfPrescan, DfSupportOutcome, DfSupportRequest, ScanCache, start_df_prescan, start_df_supports,
};
use sc_mesh_core::transform::Transform3D;
use sc_print_profiles::resolve_layer_height;
use sc_scene_edit::SceneFragment;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::preferences_bus::PreferencesBus;
use crate::print_selection::PrintDefaults;
use crate::scene_document::{SceneDocument, SceneEvent, SceneRevision, SceneSubscription};
use crate::slice_projection::load_selected_profiles;

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the scene must sit still before the prescan starts.
const PRESCAN_SETTLE: Duration = Duration::from_millis(500);

/// How often the prescan watcher looks at the scene.
const PRESCAN_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// The background prescan's state.
#[derive(Default)]
struct Prescan {
    /// The job in flight, the scene it started on and when.
    running: Option<(JobHandle<DfPrescan>, SceneRevision, Instant)>,
    /// The last finished prescan and the scene it ran on.
    ready: Option<(SceneRevision, Arc<ScanCache>)>,
}

pub(crate) struct DfSupportRun {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    handle: RefCell<Option<JobHandle<DfSupportOutcome>>>,
    // The scene the run was started on: a run that finishes on edited
    // geometry is discarded rather than pasted.
    started_on: RefCell<Option<SceneRevision>>,
    poll_timer: Rc<slint::Timer>,
    prescan: RefCell<Prescan>,
    /// When the scene last changed, live drag ticks included. Stamped by the
    /// scene subscription; that stamp is all the prescan costs an edit.
    last_change: Rc<Cell<Instant>>,
    prescan_timer: slint::Timer,
    _scene_events: SceneSubscription,
}

impl DfSupportRun {
    /// Wires the Tools menu and the panel's Cancel button; the caller keeps
    /// the driver alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
    ) -> Rc<Self> {
        let last_change = Rc::new(Cell::new(Instant::now()));
        let scene_events = document.subscribe({
            let last_change = last_change.clone();
            move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::Mutated
                        | SceneEvent::Restructured
                ) {
                    last_change.set(Instant::now());
                }
            }
        });
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            bus,
            document,
            dialogs,
            handle: RefCell::new(None),
            started_on: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
            prescan: RefCell::new(Prescan::default()),
            last_change,
            prescan_timer: slint::Timer::default(),
            _scene_events: scene_events,
        });
        {
            let weak = Rc::downgrade(&run);
            run.prescan_timer.start(
                slint::TimerMode::Repeated,
                PRESCAN_POLL_INTERVAL,
                move || {
                    if let Some(run) = weak.upgrade() {
                        run.watch_prescan();
                    }
                },
            );
        }
        {
            let run = run.clone();
            ui.on_df_support_requested(move || run.start());
        }
        {
            let run = run.clone();
            ui.on_support_gen_cancel_requested(move || {
                if let Some(handle) = run.handle.borrow().as_ref() {
                    handle.cancel();
                }
            });
        }
        run
    }

    fn start(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading() || ui.get_support_gen_in_progress() || self.handle.borrow().is_some() {
            return;
        }
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        let mut request = self.request_for_selection();
        {
            let mut prescan = self.prescan.borrow_mut();
            // The run needs the cores more than a prescan still in flight.
            if let Some((job, _, _)) = prescan.running.take() {
                job.cancel();
            }
            // A stale entry would simply not match; there is no point
            // handing it over.
            if let Some((revision, scans)) = &prescan.ready
                && !self.document.slice_geometry_changed(*revision)
            {
                request.prescanned = Some(scans.clone());
            }
        }
        match start_df_supports(&scene, request) {
            Ok(handle) => {
                ui.set_support_gen_in_progress(true);
                ui.set_support_gen_cancelable(true);
                ui.set_support_gen_progress(0.0);
                ui.set_support_gen_phase(slint::SharedString::default());
                *self.handle.borrow_mut() = Some(handle);
                *self.started_on.borrow_mut() = Some(self.document.scene_revision());
                self.start_poll();
            }
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
            }
        }
    }

    /// The prescan watcher, every [`PRESCAN_POLL_INTERVAL`]: collects a
    /// finished prescan, cancels one the scene has moved on from, and starts
    /// one once the scene has been still for [`PRESCAN_SETTLE`] and holds no
    /// matching scans yet. Idle while a support run is in flight, a scene is
    /// loading or the preference is off.
    fn watch_prescan(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let mut prescan = self.prescan.borrow_mut();
        if let Some((job, revision, _)) = &prescan.running
            && let Some(finished) = job.try_poll()
        {
            let revision = *revision;
            prescan.running = None;
            if let Ok(done) = finished {
                prescan.ready = Some((revision, done.scans));
            }
        }
        if !self.bus.snapshot().prescan_supports {
            if let Some((job, _, _)) = prescan.running.take() {
                job.cancel();
            }
            prescan.ready = None;
            return;
        }
        let last_change = self.last_change.get();
        if let Some((job, _, started)) = &prescan.running
            && last_change > *started
        {
            // The scene moved under it: its scans would not match.
            job.cancel();
            prescan.running = None;
        }
        let busy = self.handle.borrow().is_some()
            || ui.get_loading()
            || ui.get_support_gen_in_progress()
            || ui.get_slicing_in_progress();
        if busy || prescan.running.is_some() || last_change.elapsed() < PRESCAN_SETTLE {
            return;
        }
        if prescan
            .ready
            .as_ref()
            .is_some_and(|(revision, _)| !self.document.slice_geometry_changed(*revision))
        {
            return;
        }
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        let mut request = self.request_for_selection();
        // Models the last prescan already scanned are taken from it.
        request.prescanned = prescan.ready.as_ref().map(|(_, scans)| scans.clone());
        let job = start_df_prescan(&scene, request);
        prescan.running = Some((job, self.document.scene_revision(), Instant::now()));
    }

    /// The request for the printer and resin in force: the layer height the
    /// slice will use, and the printer's pixel pitch, which sets how far the
    /// contact tips sink into the model. The defaults (0.05 mm, tips at the
    /// surface) when no printer or resin is in force or a profile fails to
    /// load.
    fn request_for_selection(&self) -> DfSupportRequest {
        let snapshot = self.bus.snapshot();
        let selection = self
            .document
            .print_selection()
            .in_force(&PrintDefaults::from(&snapshot));
        let (Some(printer), Some(resin)) =
            (selection.printer().in_force(), selection.resin().in_force())
        else {
            return DfSupportRequest::default();
        };
        let Ok((printer, resin)) = load_selected_profiles(printer, resin) else {
            return DfSupportRequest::default();
        };
        let defaults = DfSupportRequest::default();
        let layer_height_mm = resolve_layer_height(&printer, &resin, None)
            .map_or(defaults.layer_height_mm, |h| h.value());
        let pitch = printer.hardware.pixel_size_microns();
        DfSupportRequest {
            layer_height_mm,
            pixel_pitch_mm: Some([pitch.x.value() / 1000.0, pitch.y.value() / 1000.0]),
            ..defaults
        }
    }

    fn start_poll(self: &Rc<Self>) {
        let run = self.clone();
        let timer_weak = Rc::downgrade(&self.poll_timer);
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let stop = || {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                };
                let Some(ui) = run.ui.upgrade() else {
                    stop();
                    return;
                };
                let finished = {
                    let slot = run.handle.borrow();
                    let Some(handle) = slot.as_ref() else {
                        stop();
                        return;
                    };
                    let progress = handle.progress();
                    ui.set_support_gen_progress(progress.fraction());
                    ui.set_support_gen_phase(progress.phase.into());
                    handle.try_poll()
                };
                if let Some(outcome) = finished {
                    stop();
                    run.handle.borrow_mut().take();
                    ui.set_support_gen_in_progress(false);
                    ui.set_support_gen_cancelable(false);
                    run.finish(outcome);
                }
            },
        );
    }

    fn finish(&self, outcome: Result<DfSupportOutcome, JobError>) {
        let started_on = self.started_on.borrow_mut().take();
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JobError::Cancelled) => return,
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
                return;
            }
        };
        if started_on.is_none_or(|revision| self.document.slice_geometry_changed(revision)) {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("DfSupportDialog", "Supports discarded"),
                message: ctr!(
                    "DfSupportDialog",
                    "The scene changed while supports were being generated; nothing was added."
                ),
                details: Default::default(),
            });
            return;
        }
        let summary = summary_message(&outcome);
        let mut details = details_message(&outcome);
        // A release build has no console (Windows GUI subsystem): the full
        // breakdown goes to a log file instead, named in the details.
        if let Some(path) = append_timing_log(&summary, &outcome.timings.report()) {
            details.push_str(&format!("\n\nFull timings: {}", path.display()));
        }
        let Some(mesh) = outcome.mesh else {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("DfSupportDialog", "No supports needed"),
                message: summary.as_str().into(),
                details: details.as_str().into(),
            });
            return;
        };
        let fragment = SceneFragment::from_leaf_snapshot(Arc::new(mesh), Transform3D::Identity);
        if let Err(error) = self.document.paste_fragment(&fragment, [0.0, 0.0, 0.0]) {
            let details = error.to_string();
            self.dialogs.show_error(move || ErrorTexts {
                heading: ctr!("DfSupportDialog", "Support generation failed"),
                context: None,
                message: ctr!(
                    "DfSupportDialog",
                    "The scene rejected the generated supports; nothing was changed."
                ),
                details: details.as_str().into(),
            });
            return;
        }
        self.dialogs.show_info(move || InfoTexts {
            heading: ctr!("DfSupportDialog", "Supports generated"),
            message: summary.as_str().into(),
            details: details.as_str().into(),
        });
    }
}

/// One line: models, islands, roots and triangles.
fn summary_message(outcome: &DfSupportOutcome) -> String {
    format!(
        "{} model(s), {} island(s), {} support root(s), {} triangle(s).",
        outcome.models,
        outcome.islands,
        outcome.support.collections.roots.len(),
        outcome.triangles
    )
}

/// Appends one run's summary and full timing report to
/// `<local data dir>/logs/df-supports-timings.log`, returning its path, or
/// `None` when there is no data directory or the write fails (the log is a
/// diagnostic: a failure never stops the run).
fn append_timing_log(summary: &str, report: &str) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let dir = sc_app_paths::project_dirs()?.data_local_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("df-supports-timings.log");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    writeln!(file, "=== run at unix {stamp}: {summary}\n{report}").ok()?;
    Some(path)
}

/// The placed supports by type, then where the time went: the stages, the
/// plan's phases and the costliest inner measurements.
fn details_message(outcome: &DfSupportOutcome) -> String {
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let mut lines: Vec<String> = outcome
        .placed
        .iter()
        .map(|(kind, count)| format!("{kind}: {count}"))
        .collect();
    let t = &outcome.timings;
    lines.push(String::new());
    lines.push(format!("Time: {:.0} ms", ms(t.total)));
    if t.entries.iter().any(|e| e.label == "scan:prescanned") {
        lines.push("  scans prepared in the background".to_owned());
    }
    for prefix in ["stage:", "auto:"] {
        for e in t.by_cost(prefix) {
            lines.push(format!("  {:<24} {:>8.0} ms", e.label, ms(e.total)));
        }
    }
    let mut inner: Vec<_> = t
        .entries
        .iter()
        .filter(|e| !e.label.starts_with("stage:") && !e.label.starts_with("auto:"))
        .collect();
    inner.sort_by_key(|e| std::cmp::Reverse(e.total));
    for e in inner.iter().take(6) {
        lines.push(format!(
            "  {:<24} {:>8.0} ms ({}x)",
            e.label,
            ms(e.total),
            e.count
        ));
    }
    lines.join("\n")
}
