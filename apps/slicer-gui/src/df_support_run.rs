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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use sc_geom::JobError;
use sc_geom::JobHandle;
use sc_geom::df_supports::{DfSupportOutcome, DfSupportRequest, start_df_supports};
use sc_mesh_core::transform::Transform3D;
use sc_scene_edit::SceneFragment;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::scene_document::{SceneDocument, SceneRevision};

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct DfSupportRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    handle: RefCell<Option<JobHandle<DfSupportOutcome>>>,
    // The scene the run was started on: a run that finishes on edited
    // geometry is discarded rather than pasted.
    started_on: RefCell<Option<SceneRevision>>,
    poll_timer: Rc<slint::Timer>,
}

impl DfSupportRun {
    /// Wires the Tools menu and the panel's Cancel button; the caller keeps
    /// the driver alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            handle: RefCell::new(None),
            started_on: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
        });
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
        match start_df_supports(&scene, DfSupportRequest::default()) {
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
        let details = details_message(&outcome);
        eprintln!("[df-supports] {summary}\n{}", outcome.timings.report());
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
