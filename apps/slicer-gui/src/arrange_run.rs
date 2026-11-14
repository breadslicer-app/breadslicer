/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Arrange all and Arrange selection (#180).
//!
//! The Transform tab's Arrange card packs the plate's objects by their real
//! footprints ([`sc_arrange`]): each model with its supports and raft, kept
//! a gap apart at every height (a part may reach over a shorter neighbour),
//! or by their whole shadow. Arrange selection finds a spot for the selected
//! model and leaves the others where they are.
//!
//! The footprints and the search run on a worker. The result lands as one
//! undoable edit that moves each model in the plate plane, maybe turned
//! about the vertical axis: its supports go along without being generated
//! again ([`crate::support_follow`]). A result worked out for a scene that
//! has changed since is dropped. Objects that do not fit go beside the
//! plate, and are named.
//!
//! Fill bed (#356, [`fill`]) leaves everything where it is and fills the
//! room left with copies of the selected models.
//!
//! Arrange all ([`across`], #554) packs every plate's models for the fewest
//! plates, adding plates as they are needed; it asks first when models were
//! placed by hand, which may stay where they are.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sc_arrange::{Across, Body, Outcome, Part, Placement, Plate, Settings};
use sc_geom::df_supports::is_any_support_mesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, HandPlacedChoice, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneRevision};
use crate::support_forecast::{ForecastCtx, with_forecasts};
use crate::support_persist::{in_force, model_of};
use crate::viewport::ViewportController;

mod across;
mod fill;

#[cfg(test)]
pub(crate) use fill::{copied_set_now, fill_now};

/// What the Arrange card sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ArrangeOptions {
    /// The smallest distance between two objects, at any height (mm).
    pub(crate) gap_mm: f32,
    /// The room kept clear along the plate's edges (mm).
    pub(crate) margin_mm: f32,
    /// Whether objects may turn about the vertical axis.
    pub(crate) turns: bool,
    /// Keep each object's whole shadow clear of the others.
    pub(crate) shadow_only: bool,
    /// Fill bed adds at most this many copies; 0 adds as many as fit.
    pub(crate) max_copies: u32,
    /// One raft for all (#556): the rafts take no room, so they lie on each
    /// other and print as one; what stands on them keeps the gap.
    pub(crate) one_raft: bool,
    /// The top of the scene's rafts when the press was made (mm): under it
    /// nothing takes room with `one_raft`.
    pub(crate) raft_top_mm: f32,
}

/// Above the rafts' top, what is still raft (its rounding, the feet's first
/// rise) takes no room either.
const ONE_RAFT_SLACK_MM: f32 = 0.05;

impl Default for ArrangeOptions {
    fn default() -> Self {
        Self {
            gap_mm: 1.0,
            margin_mm: 2.0,
            turns: true,
            shadow_only: false,
            max_copies: 0,
            one_raft: false,
            raft_top_mm: 0.0,
        }
    }
}

impl ArrangeOptions {
    pub(crate) fn settings(self) -> Settings {
        let (gap, free_below) = if self.one_raft {
            (
                self.gap_mm,
                if self.raft_top_mm > 0.0 {
                    self.raft_top_mm + ONE_RAFT_SLACK_MM
                } else {
                    0.0
                },
            )
        } else {
            (self.gap_mm, 0.0)
        };
        Settings {
            gap: f64::from(gap),
            margin: f64::from(self.margin_mm),
            shadow_only: self.shadow_only,
            turns: if self.turns { 8 } else { 1 },
            free_below: f64::from(free_below),
            // Under one raft for all the models pack tighter (#556);
            // without it Arrange and Fill bed stay as they are.
            tight: self.one_raft,
            ..Settings::default()
        }
    }

    /// These options with one raft for all as the Supports tab has it
    /// (#556), and the rafts' top of `document` as it stands.
    pub(crate) fn on(mut self, document: &SceneDocument, one_raft: bool) -> Self {
        self.one_raft = one_raft;
        self.raft_top_mm = if self.one_raft {
            let sessions = document.support_sessions();
            document
                .with_scene(|spatial| crate::support_persist::raft_top_mm(spatial, &sessions))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        self
    }
}

/// The plate's objects as the arrangement sees them.
struct Gathered {
    bodies: Vec<Body>,
    /// Per body, the leaves its move moves: the model, and supports that
    /// do not follow it by themselves. A set in force follows its model.
    leaves: Vec<Vec<NodeId>>,
    /// Per body, the support leaves whose set in force follows its model.
    follows: Vec<Vec<NodeId>>,
    names: Vec<String>,
    /// Per body, the leaf that says which plate it stands on (#545): its
    /// model, or the supports no model owns.
    anchors: Vec<NodeId>,
    /// Per body, whether it is a model with no supports yet: it is measured
    /// with the supports it will get (#556).
    bare: Vec<bool>,
}

/// A run in flight.
struct Pending {
    revision: SceneRevision,
    /// Per plate arranged (#545), each body's leaves and name.
    leaves: Vec<Vec<Vec<NodeId>>>,
    names: Vec<Vec<String>>,
    started: Instant,
}

/// Which plates an Arrange press works on (#545).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrangeScope {
    /// Every plate with objects, each on its own: the card's buttons and A.
    Plates,
    /// The active plate alone: its own tool.
    ActivePlate,
}

/// The worker's outcome: one per plate arranged.
type ArrangeSlot = Arc<Mutex<Option<Result<Option<Vec<Outcome>>, String>>>>;

/// An Arrange all's outcome (#554).
/// With it, when the worker ended: how long the window took to take it is
/// noted (#560).
type AcrossSlot = Arc<Mutex<Option<(Result<Option<Across>, String>, Instant)>>>;

/// An Arrange all in flight (#554).
struct AcrossPending {
    revision: SceneRevision,
    job: across::Job,
    started: Instant,
    /// Pressed again with nothing done to the scene since the last (#560).
    again: bool,
    /// The card's settings it works with.
    options: ArrangeOptions,
}

pub(crate) struct ArrangeRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    options: Cell<ArrangeOptions>,
    slot: ArrangeSlot,
    pending: RefCell<Option<Pending>>,
    across_slot: AcrossSlot,
    across: RefCell<Option<AcrossPending>>,
    cancel: Arc<AtomicBool>,
    fill: fill::Fill,
    ahead: Ahead,
    /// The scene the last Arrange all left, with the settings and the
    /// plates it worked with (#560).
    arranged: Cell<Option<(SceneRevision, ArrangeOptions, ArrangeScope)>>,
    /// What a model with no supports yet is measured with (#556): the
    /// Supports tab's request, from its driver.
    forecast: RefCell<Option<Rc<dyn Fn() -> ForecastCtx>>>,
}

/// The footprints drawn ahead (#560): once the scene has stood still a
/// moment, every part's bands are drawn in the background and kept, so an
/// Arrange, Fill bed or import finds them ready.
#[derive(Default)]
struct Ahead {
    timer: slint::Timer,
    /// The scene and the card's options last drawn for.
    drawn: Cell<Option<(SceneRevision, ArrangeOptions)>>,
    /// The scene the last look found: drawing waits for it to stand still.
    seen: Cell<Option<SceneRevision>>,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    /// When the timer last ticked: a tick much later means the window was
    /// held, and the diagnostic log says for how long (#560).
    ticked: Cell<Option<Instant>>,
}

/// A window held longer than this between ticks is noted in the log.
const HELD_NOTED: Duration = Duration::from_millis(1500);

/// How often the scene is looked at for footprints to draw ahead.
const AHEAD_TICK: Duration = Duration::from_millis(400);

impl ArrangeRun {
    /// Wires the Arrange card's callbacks and returns the driver, which the
    /// caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            ctrl,
            options: Cell::new(ArrangeOptions::default()),
            slot: Arc::new(Mutex::new(None)),
            pending: RefCell::new(None),
            across_slot: Arc::new(Mutex::new(None)),
            across: RefCell::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            fill: fill::Fill::default(),
            ahead: Ahead::default(),
            arranged: Cell::new(None),
            forecast: RefCell::new(None),
        });
        run.push_options(ui);
        {
            let weak = Rc::downgrade(&run);
            run.ahead
                .timer
                .start(slint::TimerMode::Repeated, AHEAD_TICK, move || {
                    if let Some(run) = weak.upgrade() {
                        let now = Instant::now();
                        if let Some(last) = run.ahead.ticked.replace(Some(now))
                            && now - last > AHEAD_TICK + HELD_NOTED
                        {
                            crate::breadcrumbs::note(&format!(
                                "window: held {:.1} s (arranging {})",
                                (now - last - AHEAD_TICK).as_secs_f64(),
                                run.ui.upgrade().is_some_and(|ui| ui.get_arranging()),
                            ));
                        }
                        run.draw_ahead();
                    }
                });
        }
        {
            let run = run.clone();
            ui.on_arrange_requested(move |selection| run.start(selection, ArrangeScope::Plates));
        }
        {
            // The active plate's own Arrange (#545): all of its objects.
            let run = run.clone();
            ui.on_plate_arrange_requested(move || run.start(false, ArrangeScope::ActivePlate));
        }
        {
            let run = run.clone();
            ui.on_arrange_finished(move || run.finish());
        }
        {
            let run = run.clone();
            ui.on_arrange_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_arrange_cancel(move || run.cancel());
        }
        {
            let run = run.clone();
            ui.on_arrange_fill_requested(move || run.start_fill());
        }
        run
    }

    /// Draws the scene's footprints in the background once it stands still,
    /// when nothing else is busy (#560).
    fn draw_ahead(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.ahead.running.load(Ordering::Relaxed)
            || !self.document.is_loaded()
            || ui.get_loading()
            || ui.get_slicing_in_progress()
            || ui.get_repairing_in_progress()
            || ui.get_arranging()
            || self.ctrl.is_mid_gesture()
        {
            return;
        }
        let now = (self.document.scene_revision(), self.options_now());
        if self.ahead.drawn.get() == Some(now) {
            return;
        }
        // The scene stood still since the last look.
        if self.ahead.seen.replace(Some(now.0)) != Some(now.0) {
            return;
        }
        self.ahead.drawn.set(Some(now));
        let Some(gathered) = gather_all(&self.document, false) else {
            return;
        };
        let plate = plate_of(&self.ctrl.build_volume());
        let settings = now.1.settings();
        let (running, cancel) = (self.ahead.running.clone(), self.ahead.cancel.clone());
        cancel.store(false, Ordering::Relaxed);
        running.store(true, Ordering::Relaxed);
        std::thread::spawn(move || {
            let started = Instant::now();
            let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sc_arrange::warm(&gathered.bodies, &plate, &settings, &cancel)
            }));
            if let Ok(drawn) = drawn
                && drawn > 0
            {
                crate::breadcrumbs::note(&format!(
                    "arrange: {drawn} part(s) drawn ahead in {:.2} s",
                    started.elapsed().as_secs_f64()
                ));
            }
            running.store(false, Ordering::Relaxed);
        });
    }

    /// The card's options as a press now uses them: with one raft for all
    /// as the Supports tab has it (#556).
    pub(crate) fn options_now(&self) -> ArrangeOptions {
        let one_raft = self
            .ui
            .upgrade()
            .is_some_and(|ui| ui.get_arrange_one_raft());
        let mut options = self.options.get().on(&self.document, one_raft);
        // Under one raft for all the raft the preset draws takes no room,
        // also before any set is made (#556).
        if options.one_raft
            && let Some(ctx) = self.forecast_now()
        {
            options.raft_top_mm = options.raft_top_mm.max(ctx.raft_top_mm as f32);
        }
        options
    }

    /// Where the supports a model will get are worked out from (#556): the
    /// Supports tab's driver.
    pub(crate) fn set_forecast(&self, source: Rc<dyn Fn() -> ForecastCtx>) {
        *self.forecast.borrow_mut() = Some(source);
    }

    /// The context models with no supports yet are measured in, when the
    /// window has a Supports tab.
    pub(crate) fn forecast_now(&self) -> Option<ForecastCtx> {
        self.forecast.borrow().as_ref().map(|source| source())
    }

    /// Stops drawing ahead: a run of its own draws what it needs.
    fn stop_ahead(&self) {
        self.ahead.cancel.store(true, Ordering::Relaxed);
    }

    fn push_options(&self, ui: &MainWindow) {
        let o = self.options.get();
        ui.set_arrange_gap(o.gap_mm);
        ui.set_arrange_margin(o.margin_mm);
        ui.set_arrange_turns(o.turns);
        ui.set_arrange_shadow_only(o.shadow_only);
        ui.set_arrange_max_copies(o.max_copies as f32);
    }

    fn on_option(&self, code: i32, value: f32) {
        let mut o = self.options.get();
        match code {
            0 => o.gap_mm = value.clamp(0.0, 20.0),
            1 => o.margin_mm = value.clamp(0.0, 50.0),
            2 => o.turns = value != 0.0,
            3 => o.shadow_only = value != 0.0,
            4 => o.max_copies = value.round().clamp(0.0, 9999.0) as u32,
            5 => {
                // One raft for all is the supports' (#556): the box here
                // stands for the one in the Supports tab.
                if let Some(ui) = self.ui.upgrade() {
                    ui.invoke_support_option_changed(14, value);
                }
                return;
            }
            _ => return,
        }
        self.options.set(o);
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
    }

    /// Arrange all, or (`selection`) the selected models alone.
    pub(crate) fn start(self: &Rc<Self>, selection: bool, scope: ArrangeScope) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading()
            || ui.get_slicing_in_progress()
            || ui.get_repairing_in_progress()
            || ui.get_arranging()
            || !self.ctrl.allows_layout_tools()
        {
            return;
        }
        if !selection {
            self.ask_then_arrange(scope);
            return;
        }
        let jobs = jobs_on_plates(&self.document, &self.ctrl.build_volume(), selection, scope);
        if jobs.is_empty() {
            return;
        }
        let settings = self.options_now().settings();
        let forecast = self.forecast_now();
        let mut leaves = Vec::new();
        let mut names = Vec::new();
        let mut work = Vec::new();
        for (gathered, plate) in jobs {
            leaves.push(gathered.leaves);
            names.push(gathered.names);
            work.push((gathered.bodies, gathered.bare, plate));
        }
        *self.pending.borrow_mut() = Some(Pending {
            revision: self.document.scene_revision(),
            leaves,
            names,
            started: Instant::now(),
        });
        self.cancel.store(false, Ordering::Relaxed);
        self.stop_ahead();
        ui.set_arranging(true);
        crate::breadcrumbs::note(&format!(
            "arrange: {} plate(s), {} object(s), selection {selection}",
            work.len(),
            work.iter().map(|(b, _, _)| b.len()).sum::<usize>()
        ));
        let slot = self.slot.clone();
        let cancel = self.cancel.clone();
        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work.into_iter()
                    .map(|(mut bodies, bare, plate)| {
                        if let Some(ctx) = &forecast {
                            with_forecasts(&mut bodies, &bare, ctx, &cancel)?;
                        }
                        sc_arrange::arrange(&bodies, &plate, &settings, &cancel).ok()
                    })
                    .collect::<Option<Vec<Outcome>>>()
            }))
            .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_arrange_finished());
        });
    }

    /// Arrange all over the plates (#554): asks first whether models placed
    /// by hand stay where they are.
    fn ask_then_arrange(self: &Rc<Self>, scope: ArrangeScope) {
        let volume = self.ctrl.build_volume();
        let count = across::by_hand(&self.document, &volume, scope);
        if count == 0 {
            self.arrange_across(scope, false);
            return;
        }
        let run = self.clone();
        self.dialogs.show_hand_placed(
            move || InfoTexts {
                heading: ctr!("ArrangeDialog", "Models placed by hand"),
                message: ctr!(
                    "ArrangeDialog",
                    "{n} model was placed by hand. Arrange all can also find the best place for it.",
                    "{n} models were placed by hand. Arrange all can also find the best place for them.",
                    count
                ),
                details: Default::default(),
            },
            move |choice| match choice {
                HandPlacedChoice::Keep => run.arrange_across(scope, true),
                HandPlacedChoice::ArrangeToo => run.arrange_across(scope, false),
                HandPlacedChoice::Cancel => {}
            },
        );
    }

    /// Starts an Arrange all of `scope` on a worker (#554); with
    /// `keep_by_hand`, the models placed by hand stay where they are.
    fn arrange_across(self: &Rc<Self>, scope: ArrangeScope, keep_by_hand: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_arranging() {
            return;
        }
        // Nothing changed since the last: as a slice still up to date, it
        // stands (#560).
        let revision = self.document.scene_revision();
        let last = self.arranged.get();
        let options = self.options_now();
        if last == Some((revision, options, scope)) {
            ui.set_arrange_last_run(ctr!(
                "Inspector",
                "Already arranged: nothing changed since the last Arrange."
            ));
            return;
        }
        let Some(job) = across::job(
            &self.document,
            &self.ctrl.build_volume(),
            scope,
            keep_by_hand,
        ) else {
            ui.set_arrange_last_run(ctr!("Inspector", "Everything is already in place."));
            return;
        };
        let settings = options.settings();
        let forecast = self.forecast_now();
        let again = last.is_some_and(|(r, _, _)| r == revision);
        let (mut bodies, bare, stands_on, plates) = (
            job.bodies.clone(),
            job.bare.clone(),
            job.stands_on.clone(),
            job.plates.clone(),
        );
        crate::breadcrumbs::note(&format!(
            "arrange: across the plates, {} object(s), keep by hand {keep_by_hand}",
            bodies.len()
        ));
        *self.across.borrow_mut() = Some(AcrossPending {
            revision: self.document.scene_revision(),
            job,
            started: Instant::now(),
            again,
            options,
        });
        self.cancel.store(false, Ordering::Relaxed);
        self.stop_ahead();
        ui.set_arranging(true);
        let slot = self.across_slot.clone();
        let cancel = self.cancel.clone();
        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(ctx) = &forecast {
                    with_forecasts(&mut bodies, &bare, ctx, &cancel)?;
                }
                sc_arrange::arrange_across(&bodies, &stands_on, &plates, &settings, &cancel).ok()
            }))
            .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((result, Instant::now()));
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_arrange_finished());
        });
    }

    /// Arrange all pressed, past the hand-placed question (#560).
    #[cfg(test)]
    pub(crate) fn arrange_all_for_test(self: &Rc<Self>) {
        self.arrange_across(ArrangeScope::Plates, false);
    }

    /// Whether an Arrange all's worker has ended, its result waiting for the
    /// window.
    #[cfg(test)]
    pub(crate) fn across_done_for_test(&self) -> bool {
        self.across_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Applies an Arrange all's result (#554), when the scene is still the
    /// one it was worked out for.
    fn finish_across(&self, ui: &MainWindow, pending: AcrossPending) {
        let taken = self
            .across_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let taken_at = Instant::now();
        let ended = taken.as_ref().map(|(_, ended)| *ended);
        let across = match taken.map(|(result, _)| result) {
            Some(Ok(Some(across))) => across,
            // Cancelled.
            None | Some(Ok(None)) => return,
            Some(Err(panic)) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Arrange failed"),
                    context: None,
                    message: ctr!("ArrangeDialog", "The arrangement stopped unexpectedly."),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        let worker = ended.map_or(Duration::ZERO, |e| {
            e.saturating_duration_since(pending.started)
        });
        let waited = ended.map_or(Duration::ZERO, |e| taken_at.saturating_duration_since(e));
        crate::breadcrumbs::note(&format!(
            "arrange: worker {:.2} s (footprints {:.2} s, search {:.2} s), the window took it {:.2} s later, {} plate(s), again {}",
            worker.as_secs_f64(),
            across.timings.footprints.as_secs_f64(),
            across.timings.search.as_secs_f64(),
            waited.as_secs_f64(),
            across.plates_used,
            pending.again,
        ));
        if self.document.scene_revision() != pending.revision {
            ui.set_arrange_last_run(ctr!(
                "Inspector",
                "The scene changed while arranging; nothing was moved."
            ));
            return;
        }
        let applying = Instant::now();
        let outcome = across::apply(&self.document, &pending.job, &across, pending.again);
        crate::breadcrumbs::note(&format!(
            "arrange: applied in {:.2} s, {} drawing(s) of supports kept, footprints kept {:.0} MB",
            applying.elapsed().as_secs_f64(),
            self.document.support_sessions().borrow().len(),
            sc_arrange::kept_bytes() as f64 / 1e6,
        ));
        let applied = match outcome {
            Ok(Some(applied)) => applied,
            Ok(None) => {
                self.arranged.set(Some((
                    self.document.scene_revision(),
                    pending.options,
                    pending.job.scope,
                )));
                ui.set_arrange_last_run(ctr!("Inspector", "Everything is already in place."));
                return;
            }
            Err(EditNotApplied::Rejected(error)) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Arrange failed"),
                    context: None,
                    message: ctr!(
                        "ArrangeDialog",
                        "The scene rejected the arrangement; nothing was moved."
                    ),
                    details: error.to_string().into(),
                });
                return;
            }
            Err(EditNotApplied::NoSceneLoaded) => return,
        };
        self.arranged.set(Some((
            self.document.scene_revision(),
            pending.options,
            pending.job.scope,
        )));
        let seconds = pending.started.elapsed().as_secs_f64();
        ui.set_arrange_last_run(across_text(&applied, seconds).into());
        if !applied.left_off.is_empty() {
            let count = applied.left_off.len();
            let names = applied.left_off.join("\n");
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("ArrangeDialog", "Not everything fits"),
                message: ctr!(
                    "ArrangeDialog",
                    "{n} object is bigger than a plate. It is beside the plates, on the right.",
                    "{n} objects are bigger than a plate. They are beside the plates, on the right.",
                    count
                ),
                details: names.as_str().into(),
            });
        }
    }

    /// Takes the worker's result on the UI thread and applies it, when the
    /// scene is still the one it was worked out for.
    fn finish(&self) {
        if self.fill.is_running() {
            self.finish_fill();
            return;
        }
        let across = self.across.borrow_mut().take();
        if let Some(pending) = across {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_arranging(false);
                self.finish_across(&ui, pending);
            }
            return;
        }
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let pending = self.pending.borrow_mut().take();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_arranging(false);
        let (Some(result), Some(pending)) = (taken, pending) else {
            return;
        };
        let outcomes = match result {
            Ok(Some(outcomes)) => outcomes,
            // Cancelled.
            Ok(None) => return,
            Err(panic) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Arrange failed"),
                    context: None,
                    message: ctr!("ArrangeDialog", "The arrangement stopped unexpectedly."),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        if self.document.scene_revision() != pending.revision {
            ui.set_arrange_last_run(ctr!(
                "Inspector",
                "The scene changed while arranging; nothing was moved."
            ));
            return;
        }
        for outcome in &outcomes {
            crate::breadcrumbs::note(&format!(
                "arrange: footprints {:?}, search {:?}",
                outcome.timings.footprints, outcome.timings.search
            ));
        }
        // Each plate's moves, in one undo step.
        let mut edits: Vec<SceneEdit> = outcomes
            .iter()
            .zip(&pending.leaves)
            .filter_map(|(outcome, leaves)| arrangement_edit(&self.document, outcome, leaves))
            .collect();
        let edit = match edits.len() {
            0 => {
                ui.set_arrange_last_run(ctr!("Inspector", "Everything is already in place."));
                return;
            }
            1 => edits.remove(0),
            _ => SceneEdit::Compound(edits),
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => self.document.push_edit(edit),
            Err(EditNotApplied::Rejected(error)) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("ArrangeDialog", "Arrange failed"),
                    context: None,
                    message: ctr!(
                        "ArrangeDialog",
                        "The scene rejected the arrangement; nothing was moved."
                    ),
                    details: error.to_string().into(),
                });
                return;
            }
            Err(EditNotApplied::NoSceneLoaded) => return,
        }
        let moved: usize = outcomes
            .iter()
            .map(|o| o.placements.iter().flatten().count())
            .sum();
        let left_off: Vec<String> = outcomes
            .iter()
            .zip(&pending.names)
            .flat_map(|(outcome, names)| {
                outcome
                    .left_off()
                    .into_iter()
                    .filter_map(|i| names.get(i).cloned())
                    .collect::<Vec<_>>()
            })
            .collect();
        let seconds = pending.started.elapsed().as_secs_f64();
        ui.set_arrange_last_run(last_run_text(moved, left_off.len(), seconds).into());
        if !left_off.is_empty() {
            let count = left_off.len();
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("ArrangeDialog", "Not everything fits"),
                message: ctr!(
                    "ArrangeDialog",
                    "{n} object does not fit on the plate. It is beside the plate, on the right.",
                    "{n} objects do not fit on the plate. They are beside the plate, on the right.",
                    count
                ),
                details: left_off.join("\n").into(),
            });
        }
    }

    /// Stops a run in flight; its result is dropped.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The plate of the printer whose build volume is `volume`.
pub(crate) fn plate_of(volume: &crate::build_volume::BuildVolume) -> Plate {
    let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
    Plate {
        min: [f64::from(lo.x), f64::from(lo.y)],
        max: [f64::from(hi.x), f64::from(hi.y)],
        floor: f64::from(lo.z),
    }
}

/// The plates' objects as they stand, each model with its supports, none of
/// them to move: what models imported into the scene keep clear of (#384),
/// with the plate each stands on (#557).
pub(crate) fn standing_bodies(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
) -> (Vec<Body>, Vec<Option<usize>>) {
    let Some(gathered) = gather_all(document, false) else {
        return (Vec::new(), Vec::new());
    };
    let plates = document.plates();
    let on: Vec<Option<usize>> = document
        .with_scene(|spatial| {
            gathered
                .anchors
                .iter()
                .map(|&leaf| {
                    crate::plate_layout::plate_of_leaf(spatial, volume, plates, leaf)
                        .map(|p| p as usize)
                })
                .collect()
        })
        .unwrap_or_default();
    let bodies = gathered
        .bodies
        .into_iter()
        .map(|body| Body {
            fixed: true,
            ..body
        })
        .collect();
    (bodies, on)
}

/// Every plate a scene may have on `volume`, in their order (#557).
pub(crate) fn every_plate(volume: &crate::build_volume::BuildVolume) -> Vec<Plate> {
    (0..crate::plates::MAX_PLATES)
        .map(|p| across::plate_at(volume, p))
        .collect()
}

/// The card's line after an Arrange all (#554).
fn across_text(applied: &across::Applied, seconds: f64) -> String {
    let time = format!("{seconds:.1}");
    let placed = applied.moved - applied.left_off.len();
    let mut line = ctr!(
        "Inspector",
        "Arranged {n} object on {} plate(s) in {} s.",
        "Arranged {n} objects on {} plate(s) in {} s.",
        placed,
        applied.plates,
        time
    )
    .to_string();
    if applied.plates > applied.plates_before {
        line.push(' ');
        line.push_str(&ctr!(
            "Inspector",
            "{n} plate added.",
            "{n} plates added.",
            applied.plates - applied.plates_before
        ));
    } else if applied.plates < applied.plates_before {
        line.push(' ');
        line.push_str(&ctr!(
            "Inspector",
            "{n} empty plate taken off.",
            "{n} empty plates taken off.",
            applied.plates_before - applied.plates
        ));
    }
    line
}

/// The card's line after a run.
fn last_run_text(moved: usize, left_off: usize, seconds: f64) -> String {
    let time = format!("{seconds:.1}");
    if left_off == 0 {
        ctr!(
            "Inspector",
            "Arranged {n} object in {} s.",
            "Arranged {n} objects in {} s.",
            moved,
            time
        )
        .to_string()
    } else {
        ctr!(
            "Inspector",
            "{} of {} objects fit; the rest are beside the plate.",
            moved - left_off,
            moved
        )
        .to_string()
    }
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

/// The plate's objects: one body per model, with its supports. Supports
/// whose model is unknown stay where they are, and so does everything but
/// the selected models when arranging the selection (#13). `None` when
/// nothing would move.
fn gather(
    document: &SceneDocument,
    selection: bool,
    on: Option<(crate::build_volume::BuildVolume, crate::plates::Plates, u32)>,
) -> Option<Gathered> {
    let mut gathered = gather_all(document, selection)?;
    if let Some((volume, plates, plate)) = on {
        // The objects of one plate (#545): each body by its first leaf, a
        // model or the supports no model owns.
        let keep: Vec<bool> = document
            .with_scene(|spatial| {
                gathered
                    .anchors
                    .iter()
                    .map(|&leaf| {
                        crate::plate_layout::plate_of_leaf(spatial, &volume, plates, leaf)
                            == Some(plate)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut i = 0;
        gathered.bodies.retain(|_| {
            i += 1;
            keep.get(i - 1).copied().unwrap_or(false)
        });
        for list in [&mut gathered.leaves, &mut gathered.follows] {
            let mut i = 0;
            list.retain(|_| {
                i += 1;
                keep.get(i - 1).copied().unwrap_or(false)
            });
        }
        let mut i = 0;
        gathered.names.retain(|_| {
            i += 1;
            keep.get(i - 1).copied().unwrap_or(false)
        });
        let mut i = 0;
        gathered.bare.retain(|_| {
            i += 1;
            keep.get(i - 1).copied().unwrap_or(false)
        });
    }
    Some(gathered).filter(|g| g.bodies.iter().any(|b| !b.fixed))
}

/// [`gather`], also when nothing would move.
fn gather_all(document: &SceneDocument, selection: bool) -> Option<Gathered> {
    let sessions = document.support_sessions();
    let selected = document.selection_node_ids();
    document.with_scene(|spatial| {
        let names = crate::viewport::object_names(spatial.graph());
        let rows: Vec<NodeId> = (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect();
        let mut gathered = Gathered {
            bodies: Vec::new(),
            leaves: Vec::new(),
            follows: Vec::new(),
            names: Vec::new(),
            anchors: Vec::new(),
            bare: Vec::new(),
        };
        let mut body_of: HashMap<NodeId, usize> = HashMap::new();
        let part = |leaf: NodeId| {
            let query = spatial.leaf(leaf)?;
            Some(Part {
                mesh: spatial.graph().leaf_mesh_arc(leaf)?.clone(),
                world: affine(&query.world_transform()),
            })
        };
        let name = |row: usize| names.get(row).cloned().unwrap_or_default();
        for (row, &leaf) in rows.iter().enumerate() {
            let Some(query) = spatial.leaf(leaf) else {
                continue;
            };
            if is_any_support_mesh(query.mesh()) {
                continue;
            }
            let Some(part) = part(leaf) else {
                continue;
            };
            body_of.insert(leaf, gathered.bodies.len());
            gathered.bodies.push(Body {
                parts: vec![part],
                fixed: selection && !selected.contains(&leaf),
            });
            gathered.leaves.push(vec![leaf]);
            gathered.follows.push(Vec::new());
            gathered.names.push(name(row));
            gathered.anchors.push(leaf);
            gathered.bare.push(true);
        }
        for (row, &leaf) in rows.iter().enumerate() {
            let Some(query) = spatial.leaf(leaf) else {
                continue;
            };
            // One raft for all (#556) is drawn again where its sets go.
            if !is_any_support_mesh(query.mesh())
                || sc_geom::df_supports::is_plate_raft(query.mesh())
            {
                continue;
            }
            let Some(support) = part(leaf) else {
                continue;
            };
            // The supports a model came with (#259) belong to the model
            // they stand under.
            let imported = sc_geom::df_supports::is_imported_supports(query.mesh());
            let model = if imported {
                crate::imported_supports::owner_of(spatial, leaf, None)
            } else {
                model_of(&sessions, leaf)
            };
            match model.and_then(|m| body_of.get(&m).copied()) {
                Some(i) => {
                    gathered.bare[i] = false;
                    gathered.bodies[i].parts.push(support);
                    // A set in force is redrawn where its model goes, and
                    // imported supports follow it; any other moves with it
                    // as it is.
                    let follows = imported
                        || in_force(spatial, &sessions, leaf)
                            .is_some_and(|drawn| Some(drawn.model) == model);
                    if follows {
                        gathered.follows[i].push(leaf);
                    } else {
                        gathered.leaves[i].push(leaf);
                    }
                }
                None => {
                    gathered.bodies.push(Body {
                        parts: vec![support],
                        fixed: true,
                    });
                    gathered.leaves.push(Vec::new());
                    gathered.follows.push(Vec::new());
                    gathered.names.push(name(row));
                    gathered.anchors.push(leaf);
                    gathered.bare.push(false);
                }
            }
        }
        gathered
    })
}

/// What an Arrange press works on: the objects, and the plate they go on.
/// With several plates (#545), each plate's objects on it, plate by plate:
/// every plate with objects to move, or the active one alone.
fn jobs_on_plates(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    selection: bool,
    scope: ArrangeScope,
) -> Vec<(Gathered, Plate)> {
    let plates = document.plates();
    if plates.count() == 1 {
        return gather(document, selection, None)
            .map(|g| vec![(g, plate_of(volume))])
            .unwrap_or_default();
    }
    let which: Vec<u32> = match scope {
        ArrangeScope::ActivePlate => vec![plates.active()],
        ArrangeScope::Plates => (0..plates.count()).collect(),
    };
    which
        .into_iter()
        .filter_map(|p| {
            let gathered = gather(document, selection, Some((*volume, plates, p)))?;
            let mut plate = plate_of(volume);
            let at = crate::plates::offset(volume, p);
            let (dx, dy) = (f64::from(at.x), f64::from(at.y));
            plate.min = [plate.min[0] + dx, plate.min[1] + dy];
            plate.max = [plate.max[0] + dx, plate.max[1] + dy];
            Some((gathered, plate))
        })
        .collect()
}

/// What a run on the active plate of `document` on `volume` takes (#548): the
/// plate to gather the objects of, with several, and the area it covers.
pub(crate) fn active_plate_job(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
) -> (
    Option<(crate::build_volume::BuildVolume, crate::plates::Plates, u32)>,
    Plate,
) {
    let plates = document.plates();
    let mut plate = plate_of(volume);
    if plates.count() == 1 {
        return (None, plate);
    }
    let at = crate::plates::offset(volume, plates.active());
    let (dx, dy) = (f64::from(at.x), f64::from(at.y));
    plate.min = [plate.min[0] + dx, plate.min[1] + dy];
    plate.max = [plate.max[0] + dx, plate.max[1] + dy];
    (Some((*volume, plates, plates.active())), plate)
}

/// A Fill press on this thread, on the active plate of `document` on
/// `volume` (#548), applied as a run applies it: for tests.
#[cfg(test)]
pub(crate) fn fill_active_plate_now(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    options: ArrangeOptions,
) -> Option<usize> {
    let (on, plate) = active_plate_job(document, volume);
    fill::fill_on_now(document, on, &plate, options).map(|(filled, _)| filled.copies.len())
}

/// An Arrange all on this thread, on the plates of `document` on `volume`
/// (#554), applied as a run applies it: for tests.
#[cfg(test)]
pub(crate) fn arrange_on_plates_now(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    options: ArrangeOptions,
    scope: ArrangeScope,
) -> Option<across::Applied> {
    arrange_across_now(document, volume, options, scope, false)
}

/// [`arrange_on_plates_now`], the models placed by hand staying where they
/// are with `keep_by_hand`.
#[cfg(test)]
pub(crate) fn arrange_across_now(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    options: ArrangeOptions,
    scope: ArrangeScope,
    keep_by_hand: bool,
) -> Option<across::Applied> {
    let job = across::job(document, volume, scope, keep_by_hand)?;
    let across = sc_arrange::arrange_across(
        &job.bodies,
        &job.stands_on,
        &job.plates,
        &options.settings(),
        &AtomicBool::new(false),
    )
    .ok()?;
    across::apply(document, &job, &across, false).ok().flatten()
}

/// [`arrange_across_now`] pressed again with other settings and nothing
/// done to the scene since (#560): its step joins the last.
#[cfg(test)]
pub(crate) fn arrange_again_now(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    options: ArrangeOptions,
) -> Option<across::Applied> {
    let job = across::job(document, volume, ArrangeScope::Plates, false)?;
    let across = sc_arrange::arrange_across(
        &job.bodies,
        &job.stands_on,
        &job.plates,
        &options.settings(),
        &AtomicBool::new(false),
    )
    .ok()?;
    across::apply(document, &job, &across, true).ok().flatten()
}

/// How many models placed by hand an Arrange of `scope` asks about: for
/// tests.
#[cfg(test)]
pub(crate) fn placed_by_hand(
    document: &SceneDocument,
    volume: &crate::build_volume::BuildVolume,
    scope: ArrangeScope,
) -> usize {
    across::by_hand(document, volume, scope)
}

/// Arranges on this thread and applies the result, as a run does: for
/// tests.
#[cfg(test)]
pub(crate) fn arrange_now(
    document: &SceneDocument,
    plate: &Plate,
    options: ArrangeOptions,
    selection: bool,
) -> Option<Outcome> {
    let gathered = gather(document, selection, None)?;
    let outcome = sc_arrange::arrange(
        &gathered.bodies,
        plate,
        &options.settings(),
        &AtomicBool::new(false),
    )
    .ok()?;
    let edit = arrangement_edit(document, &outcome, &gathered.leaves)?;
    document.apply_edit(edit.clone()).ok()?;
    document.push_edit(edit);
    Some(outcome)
}

/// One edit that moves every placed body's leaves: one undo step.
fn arrangement_edit(
    document: &SceneDocument,
    outcome: &Outcome,
    leaves: &[Vec<NodeId>],
) -> Option<SceneEdit> {
    let mut edits = motion_edits(document, &outcome.placements, leaves);
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(SceneEdit::Compound(edits)),
    }
}

/// The edits that move each placed body's leaves where `placements` say.
fn motion_edits(
    document: &SceneDocument,
    placements: &[Option<Placement>],
    leaves: &[Vec<NodeId>],
) -> Vec<SceneEdit> {
    document
        .with_scene(|spatial| {
            let mut edits = Vec::new();
            for (placement, leaves) in placements.iter().zip(leaves) {
                let Some(placement) = placement else {
                    continue;
                };
                let motion = Transform3D::Affine(placement.motion());
                for &leaf in leaves {
                    let Some(query) = spatial.leaf(leaf) else {
                        continue;
                    };
                    let before = query.local_transform();
                    let Ok(up) = query.parent_world().inverse() else {
                        continue;
                    };
                    let after = up.compose(&motion.compose(&query.world_transform()));
                    if after != before {
                        edits.push(SceneEdit::SetLocalTransform {
                            node_id: leaf,
                            before,
                            after,
                        });
                    }
                }
            }
            edits
        })
        .unwrap_or_default()
}

/// Arrange all, again and again, on a real project (#560): how long each
/// step takes, the ones on the window's thread apart.
/// `BREADSLICER_ARRANGE_BREAD` names the project, `BREADSLICER_ARRANGE_RUNS`
/// how many runs (4).
#[cfg(test)]
mod real_scene {
    use std::time::Instant;

    use super::*;

    /// The process's working set, in MB.
    fn memory_mb() -> f64 {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).WorkingSet64", std::process::id()),
            ])
            .output()
            .ok();
        out.and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
        })
        .map_or(0.0, |b| b / 1e6)
    }

    #[test]
    #[ignore = "a measurement on a real project"]
    fn arrange_all_again_and_again() {
        let Some(path) = std::env::var_os("BREADSLICER_ARRANGE_BREAD") else {
            return;
        };
        let runs: usize = std::env::var("BREADSLICER_ARRANGE_RUNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let path = std::path::PathBuf::from(path);
        crate::test_support::slint_thread::on_fresh_slint_thread(move || {
            let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("it opens");
            let sc_project_formats::LoadedContent::Project(mut project) = loaded.content else {
                panic!("a .bread is a project")
            };
            let restored =
                crate::support_persist::restore_supports(&mut project.scene, project.supports);
            println!("sets restored: {}", restored.sets.len());
            let window = MainWindow::new().expect("a main window");
            let bus = crate::preferences_bus::PreferencesBus::new(
                crate::preferences_bus::PreferencesSnapshot::for_tests(),
            );
            let ctrl = Rc::new(ViewportController::new_for_test());
            let volume = crate::build_volume::BuildVolume::new(223.642, 126.976, 230.0);
            ctrl.set_build_volume(volume);
            let document = ctrl.document();
            document.install_graph(project.scene, path.clone());
            crate::support_persist::install_sets(&document, restored.sets);
            let _supports = crate::df_support_run::DfSupportRun::install(
                &window,
                bus,
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            // The real renderer, when there is a GPU: what the window uploads
            // and works out after each arrangement.
            let mut renderer =
                crate::test_support::gpu_devices::shared_test_device().map(|(device, queue)| {
                    let r = crate::mesh_render::Renderer::new(
                        device.clone(),
                        crate::mesh_render::DeviceToken::next(),
                        queue.clone(),
                        crate::mesh_render::DeviceLoss::default(),
                    )
                    .expect("a renderer");
                    (r, device, queue)
                });
            let mut draw = |label: &str| {
                let Some((r, device, queue)) = renderer.as_mut() else {
                    return;
                };
                let t = Instant::now();
                document
                    .with_scene(|s| r.upload_scene(s.graph()))
                    .expect("scene")
                    .expect("upload");
                queue.submit(std::iter::empty());
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
                let uploaded = t.elapsed();
                let t = Instant::now();
                document.with_scene(|spatial| {
                    r.refresh_world_boxes_for_test(spatial.graph(), volume, true, false);
                });
                println!(
                    "  {label}: upload {:.2} s, world boxes {:.2} s, memory {:.0} MB",
                    uploaded.as_secs_f64(),
                    t.elapsed().as_secs_f64(),
                    memory_mb()
                );
            };
            draw("opened");
            let distinct = document
                .with_scene(|spatial| {
                    let mut seen = std::collections::HashSet::new();
                    let mut verts = 0usize;
                    for leaf in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
                        let Some(q) = spatial.leaf(leaf) else {
                            continue;
                        };
                        if is_any_support_mesh(q.mesh()) {
                            continue;
                        }
                        let mesh = spatial.graph().leaf_mesh_arc(leaf).expect("mesh");
                        if seen.insert(Arc::as_ptr(mesh) as usize) {
                            verts += mesh.vertices().len();
                        }
                    }
                    (seen.len(), verts)
                })
                .unwrap_or_default();
            println!(
                "model meshes: {} distinct, {} vertices in all",
                distinct.0, distinct.1
            );
            let _arrange =
                ArrangeRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            // The window's own work for `secs`: how long its timers kept it.
            let idle = |secs: f64| {
                let (start, mut busy) = (Instant::now(), 0.0);
                while start.elapsed().as_secs_f64() < secs {
                    let t = Instant::now();
                    slint::platform::update_timers_and_animations();
                    busy += t.elapsed().as_secs_f64();
                    std::thread::sleep(Duration::from_millis(20));
                }
                busy
            };
            // The gap of each run, as the card would set it, and how long the
            // window stands still before each.
            let gaps: Vec<f32> = std::env::var("BREADSLICER_ARRANGE_GAPS")
                .ok()
                .map(|v| v.split(',').filter_map(|g| g.parse().ok()).collect())
                .unwrap_or_default();
            let wait: f64 = std::env::var("BREADSLICER_ARRANGE_WAIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3.0);
            // One raft for all per run (#556): `1,0` arranges under one
            // raft first, then each on its own.
            let one_raft: Vec<bool> = std::env::var("BREADSLICER_ARRANGE_ONE_RAFT")
                .ok()
                .map(|v| v.split(',').map(|k| k.trim() == "1").collect())
                .unwrap_or_default();
            for run in 0..runs {
                let options = ArrangeOptions {
                    gap_mm: gaps.get(run).copied().unwrap_or(1.0),
                    one_raft: one_raft.get(run).copied().unwrap_or(false),
                    ..ArrangeOptions::default()
                };
                _arrange.options.set(options);
                let options = options.on(&document, options.one_raft);
                let settings = options.settings();
                let busy = idle(wait);
                let triangles: usize = document
                    .with_scene(|spatial| {
                        (0..)
                            .map_while(|i| spatial.leaf_id_at_index(i))
                            .filter_map(|leaf| spatial.leaf(leaf))
                            .filter(|q| is_any_support_mesh(q.mesh()))
                            .map(|q| q.mesh().faces().len())
                            .sum()
                    })
                    .unwrap_or(0);
                println!(
                    "before run {run} (gap {}, one raft {} at {:.2} mm): window busy {busy:.2} s of {wait} s, {} drawings kept, supports {:.1} M triangles, footprints kept {} ({:.0} MB), memory {:.0} MB",
                    options.gap_mm,
                    options.one_raft,
                    options.raft_top_mm,
                    document.support_sessions().borrow().len(),
                    triangles as f64 / 1e6,
                    sc_arrange::kept_parts(),
                    sc_arrange::kept_bytes() as f64 / 1e6,
                    memory_mb(),
                );
                let t = Instant::now();
                let job =
                    across::job(&document, &volume, ArrangeScope::Plates, false).expect("a job");
                let gathered = t.elapsed();
                let across = sc_arrange::arrange_across(
                    &job.bodies,
                    &job.stands_on,
                    &job.plates,
                    &settings,
                    &AtomicBool::new(false),
                )
                .expect("arranged");
                let t = Instant::now();
                // Again, as Arrange all pressed again with other settings:
                // joined to the last step.
                let again = run > 0;
                let applied = across::apply(&document, &job, &across, again).expect("applies");
                let applying = t.elapsed();
                draw("after");
                println!(
                    "  spread {:.0} mm², undo steps {}",
                    across.spread,
                    document.undo_len()
                );
                println!(
                    "run {run}: gather {:.2} s, footprints {:.2} s, search {:.2} s, apply {:.2} s; {} bodies, {} plate(s), {:?}",
                    gathered.as_secs_f64(),
                    across.timings.footprints.as_secs_f64(),
                    across.timings.search.as_secs_f64(),
                    applying.as_secs_f64(),
                    job.bodies.len(),
                    across.plates_used,
                    applied.map(|a| a.moved),
                );
            }
        });
    }
}

#[cfg(test)]
mod again_tests {
    use std::time::Duration;

    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::{LeafPayload, Scene};

    use super::*;

    /// Arrange all on boxes scattered over `ctrl`'s plate, its driver.
    fn scattered(window: &MainWindow) -> (Rc<ViewportController>, Rc<ArrangeRun>) {
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.set_build_volume(crate::build_volume::BuildVolume::new(100.0, 60.0, 80.0));
        let mut g = Scene::new();
        let mesh = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        for k in 0..5 {
            let at = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                [[6.0, 0.0, 0.0], [0.0, 6.0, 0.0], [0.0, 0.0, 6.0]],
                [k as f64 * 17.0 - 40.0, (k % 2) as f64 * 40.0 - 20.0, 3.0],
            ));
            let leaf = g
                .add_leaf(None, at, LeafPayload::Mesh(mesh.clone()))
                .unwrap();
            g.add_root(leaf).unwrap();
        }
        ctrl.install_graph_for_test(g);
        let run = ArrangeRun::install(window, ctrl.document(), ctrl.dialogs(), ctrl.clone());
        (ctrl, run)
    }

    /// Presses Arrange all and lets its worker finish.
    fn press(run: &Rc<ArrangeRun>) {
        run.arrange_across(ArrangeScope::Plates, false);
        let start = Instant::now();
        while run.across.borrow().is_some()
            && run
                .across_slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
            && start.elapsed() < Duration::from_secs(20)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        run.finish();
    }

    /// #560: Arrange all pressed again with nothing changed answers at once
    /// and adds no step; with other settings it arranges again, in the same
    /// step, and one Undo goes back to before the first.
    #[test]
    fn arrange_all_again_answers_at_once_or_joins_its_step() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("a main window");
            let (ctrl, run) = scattered(&window);
            let document = ctrl.document();
            let steps = document.undo_len();
            press(&run);
            assert_eq!(document.undo_len(), steps + 1, "arranged");
            let after = document.scene_revision();

            press(&run);
            assert!(!window.get_arranging(), "nothing ran");
            assert!(
                window.get_arrange_last_run().contains("Already arranged"),
                "{}",
                window.get_arrange_last_run()
            );
            assert_eq!(document.scene_revision(), after, "nothing moved");
            assert_eq!(document.undo_len(), steps + 1);

            // Another gap: arranged again, in the same step.
            run.on_option(0, 3.0);
            press(&run);
            assert_eq!(document.undo_len(), steps + 1, "one step");
            assert!(document.undo().unwrap());
            assert_eq!(document.undo_len(), steps, "back to before the first");
        });
    }
}
