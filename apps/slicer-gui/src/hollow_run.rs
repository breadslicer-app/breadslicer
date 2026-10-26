/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Hollow tab (SC-18): hollows the selected model with the DragonFruit
//! engine port (`sc_geom::df_hollow`) and drills drains where the model is
//! clicked.
//!
//! A hollow replaces the leaf's mesh as one undoable edit. The driver keeps,
//! per hollow mesh it installed, the original mesh and the drains (in the
//! leaf's local frame, so moving the model keeps them in place). A new drain
//! or other settings hollow the original again; Undo puts the previous mesh
//! back, and the entry it matches (by mesh identity) follows. A job that
//! finishes on a scene edited meanwhile is discarded.
//!
//! While the tab is open the selected model shows previews over it: the
//! cavity the current settings carve (DragonFruit's fast preview, recomputed
//! as the wall or detail change and re-posed as the model moves), its drains,
//! and with the drain tool the drain a click would drill.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_geom::df_hollow::hollow_df::{GridRequest, HollowFill, HollowSettings, InfillMode};
use sc_geom::df_hollow::{
    HollowKind, HollowOutcome, HollowRequest, HollowResult, PreparedHollow, WorldDrain,
    start_hollow_mesh,
};
use sc_geom::df_supports::DF_SUPPORT_MESH_NAME;
use sc_geom::{JobError, JobHandle};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_scene::NodeId;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_render::{OverlayMesh, OverlayTint};
use crate::scene_document::{
    SceneDocument, SceneEvent, SceneRevision, SceneSubscription, leaf_mesh_swap,
};
use crate::viewport::ViewportController;
use crate::{MainWindow, WorkspaceSection};

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How often the previews look at the tab, the selection and the settings.
const PREVIEW_TICK: Duration = Duration::from_millis(100);
/// How long the settings rest before the cavity is computed again.
const PREVIEW_SETTLE: Duration = Duration::from_millis(150);
/// Sides of a drawn drain.
const DRAIN_SIDES: u32 = 24;

/// The Hollow tab's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HollowOptions {
    wall_mm: f32,
    voxel_mm: f32,
    /// 0 empty, 1 lattice, 2 pillars.
    fill: u8,
    cell_mm: f32,
    beam_mm: f32,
    smooth: bool,
    drain_radius_mm: f32,
}

impl Default for HollowOptions {
    fn default() -> Self {
        // DragonFruit's defaults.
        Self {
            wall_mm: 2.0,
            voxel_mm: 0.65,
            fill: 0,
            cell_mm: 4.24,
            beam_mm: 0.35,
            smooth: true,
            drain_radius_mm: 1.5,
        }
    }
}

impl HollowOptions {
    fn settings(&self) -> HollowSettings {
        let fill = match self.fill {
            0 => HollowFill::Cavity,
            n => HollowFill::Infill {
                pattern: if n == 1 {
                    InfillMode::Lattice
                } else {
                    InfillMode::Pillar
                },
                cell_mm: self.cell_mm,
                beam_radius_mm: self.beam_mm,
            },
        };
        HollowSettings {
            fill,
            wall_thickness_mm: self.wall_mm,
            smooth_internal_surfaces: self.smooth,
            ..HollowSettings::default()
        }
    }
}

/// A drain in the leaf's local frame: a point on the surface and the
/// direction into the model.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LocalDrain {
    center: [f32; 3],
    direction: [f32; 3],
    radius_mm: f32,
}

impl LocalDrain {
    fn to_world(self, world: &Transform3D) -> WorldDrain {
        let at = |p: [f32; 3]| world.apply_to_vertex(Vertex::new(p)).coords();
        let center = at(self.center);
        let ahead = at(std::array::from_fn(|i| self.center[i] + self.direction[i]));
        WorldDrain::new(
            center,
            normalized(std::array::from_fn(|i| ahead[i] - center[i])),
            self.radius_mm,
        )
    }
}

/// A hollow this driver installed: the mesh it put on the leaf, what it was
/// made from and with which drains.
struct Hollowed {
    hollow: Arc<IndexedMesh>,
    original: Arc<IndexedMesh>,
    drains: Vec<LocalDrain>,
}

/// The job in flight and what it needs to land.
struct Job {
    handle: JobHandle<HollowOutcome>,
    leaf: NodeId,
    started_on: SceneRevision,
    /// The leaf's mesh when the job started: the edit's `before`.
    before: Arc<IndexedMesh>,
    original: Arc<IndexedMesh>,
    drains: Vec<LocalDrain>,
}

pub(crate) struct HollowRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    job: RefCell<Option<Job>>,
    poll_timer: Rc<slint::Timer>,
    hollowed: RefCell<Vec<Hollowed>>,
    options: RefCell<HollowOptions>,
    /// The last voxel session, reused when the next job matches it (a
    /// moved or rotated model, a new drain).
    prepared: RefCell<Option<Arc<PreparedHollow>>>,
    _scene_events: SceneSubscription,
    ctrl: Rc<ViewportController>,
    preview: RefCell<CavityPreview>,
    /// When an option last changed: the cavity waits for them to rest.
    options_changed: Cell<Instant>,
    /// Where the cursor rests on the model with the drain tool armed.
    hover: Cell<Option<[f32; 3]>>,
    preview_timer: slint::Timer,
}

/// What the cavity preview was computed for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PreviewKey {
    leaf: NodeId,
    /// The mesh hollowed (the original of a hollow), by identity.
    mesh: usize,
    options: HollowOptions,
}

/// The cavity preview: the job computing one, the last one computed (in the
/// leaf's local frame) and the overlay last shown from it.
#[derive(Default)]
struct CavityPreview {
    job: Option<(JobHandle<HollowOutcome>, PreviewKey)>,
    ready: Option<(PreviewKey, Arc<IndexedMesh>)>,
    /// What the shown overlay was drawn from, and the overlay.
    shown: Option<(ShownKey, Arc<[OverlayMesh]>)>,
}

/// Everything the overlay is drawn from.
#[derive(Debug, Clone, PartialEq)]
struct ShownKey {
    cavity: Option<usize>,
    world: [[f64; 4]; 4],
    drains: Vec<LocalDrain>,
    hover: Option<[f32; 3]>,
    radius: f32,
    wall: f32,
}

impl HollowRun {
    /// Wires the Hollow tab; the caller keeps the driver alive for the
    /// window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let scene_events = document.subscribe(move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::SelectionChanged
                        | SceneEvent::Mutated
                        | SceneEvent::Restructured
                ) {
                    // Out of the notification: the refresh reads the scene.
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(run) = weak.upgrade() {
                            run.refresh_ui();
                        }
                    });
                }
            });
            Self {
                ui: ui.as_weak(),
                document: document.clone(),
                dialogs,
                job: RefCell::new(None),
                poll_timer: Rc::new(slint::Timer::default()),
                hollowed: RefCell::new(Vec::new()),
                options: RefCell::new(HollowOptions::default()),
                prepared: RefCell::new(None),
                _scene_events: scene_events,
                ctrl,
                preview: RefCell::new(CavityPreview::default()),
                options_changed: Cell::new(Instant::now()),
                hover: Cell::new(None),
                preview_timer: slint::Timer::default(),
            }
        });
        run.push_options(ui);
        {
            let weak = Rc::downgrade(&run);
            run.preview_timer
                .start(slint::TimerMode::Repeated, PREVIEW_TICK, move || {
                    if let Some(run) = weak.upgrade() {
                        run.tick_preview();
                    }
                });
        }
        {
            let weak = Rc::downgrade(&run);
            ui.on_hollow_drain_hover(move |x, y, z, hit| {
                if let Some(run) = weak.upgrade() {
                    run.hover.set(hit.then_some([x, y, z]));
                    run.tick_preview();
                }
            });
        }
        {
            let run = run.clone();
            ui.on_hollow_apply(move || run.on_apply());
        }
        {
            let run = run.clone();
            ui.on_hollow_cancel(move || {
                if let Some(job) = run.job.borrow().as_ref() {
                    job.handle.cancel();
                }
            });
        }
        {
            let run = run.clone();
            ui.on_hollow_restore(move || run.on_restore());
        }
        {
            let run = run.clone();
            ui.on_hollow_clear_drains(move || run.on_clear_drains());
        }
        {
            let run = run.clone();
            ui.on_hollow_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_hollow_drain_clicked(move |x, y, z| run.on_drain_click([x, y, z]));
        }
        run
    }

    /// The selected leaf, its mesh and its world transform.
    fn selected(&self) -> Option<(NodeId, Arc<IndexedMesh>, Transform3D)> {
        let leaf = self.document.selection().primary()?.0;
        self.document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                let mesh = scene.leaf_mesh_arc(leaf)?.clone();
                let world = scene.world_transform_of(leaf).ok()?;
                Some((leaf, mesh, world))
            })
            .flatten()
    }

    /// 0 a solid model, 1 a hollow this driver made, 2 the support mesh.
    fn model_state(&self, mesh: &Arc<IndexedMesh>) -> i32 {
        if mesh.meta().as_ref().and_then(|m| m.name.as_deref()) == Some(DF_SUPPORT_MESH_NAME) {
            2
        } else if self.entry_for(mesh).is_some() {
            1
        } else {
            0
        }
    }

    /// The entry of a hollow mesh, by identity.
    fn entry_for(&self, mesh: &Arc<IndexedMesh>) -> Option<(Arc<IndexedMesh>, Vec<LocalDrain>)> {
        self.hollowed
            .borrow()
            .iter()
            .find(|h| Arc::ptr_eq(&h.hollow, mesh))
            .map(|h| (h.original.clone(), h.drains.clone()))
    }

    fn refresh_ui(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (state, drains) = match self.selected() {
            Some((_, mesh, _)) => (
                self.model_state(&mesh),
                self.entry_for(&mesh).map_or(0, |(_, d)| d.len()),
            ),
            None => (0, 0),
        };
        ui.set_hollow_model_state(state);
        ui.set_hollow_drain_count(drains as i32);
        if state != 1 {
            ui.set_hollow_drain_tool(false);
        }
    }

    fn push_options(&self, ui: &MainWindow) {
        let o = *self.options.borrow();
        ui.set_hollow_wall(o.wall_mm);
        ui.set_hollow_voxel(o.voxel_mm);
        ui.set_hollow_fill_index(i32::from(o.fill));
        ui.set_hollow_cell(o.cell_mm);
        ui.set_hollow_beam(o.beam_mm);
        ui.set_hollow_smooth(o.smooth);
        ui.set_hollow_drain_radius(o.drain_radius_mm);
    }

    /// An option change from the tab: 0 wall, 1 voxel, 2 smooth, 3 fill,
    /// 4 infill cell, 5 beam radius, 6 drain radius. Values are clamped to
    /// what the engine accepts.
    fn on_option(&self, code: i32, value: f32) {
        {
            let mut o = self.options.borrow_mut();
            match code {
                0 => o.wall_mm = value.clamp(0.5, 10.0),
                1 => o.voxel_mm = value.clamp(0.1, 3.0),
                2 => o.smooth = value != 0.0,
                3 => o.fill = (value.round() as i32).clamp(0, 2) as u8,
                4 => o.cell_mm = value.clamp(1.0, 30.0),
                5 => o.beam_mm = value.clamp(0.1, 5.0),
                6 => o.drain_radius_mm = value.clamp(0.3, 10.0),
                _ => return,
            }
        }
        self.options_changed.set(Instant::now());
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
    }

    fn on_apply(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        // Hollowing a hollow again starts from its original, keeping drains.
        let (original, drains) = self
            .entry_for(&mesh)
            .unwrap_or_else(|| (mesh.clone(), Vec::new()));
        self.launch(leaf, mesh, original, drains, &world);
    }

    fn on_restore(&self) {
        let Some((leaf, mesh, _)) = self.selected() else {
            return;
        };
        let Some((original, _)) = self.entry_for(&mesh) else {
            return;
        };
        self.swap_mesh(leaf, mesh, original);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(ctr!(
                "HollowRun",
                "Solid again. Undo brings the hollow back."
            ));
        }
    }

    fn on_clear_drains(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        let Some((original, drains)) = self.entry_for(&mesh) else {
            return;
        };
        if !drains.is_empty() {
            self.launch(leaf, mesh, original, Vec::new(), &world);
        }
    }

    /// A click with the drain tool at a world point on the selected model
    /// (the viewport selects the model it hit first).
    fn on_drain_click(self: &Rc<Self>, point: [f32; 3]) {
        if self.job.borrow().is_some() {
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        let Some((original, mut drains)) = self.entry_for(&mesh) else {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("HollowRun", "Hollow the model first"),
                message: ctr!(
                    "HollowRun",
                    "Drains are drilled into a hollow model; hollow this one, then click where the drain goes."
                ),
                details: Default::default(),
            });
            return;
        };
        let Ok(inverse) = world.inverse() else {
            return;
        };
        let local = inverse.apply_to_vertex(Vertex::new(point)).coords();
        let Some((center, normal)) = closest_surface_point(&original, local) else {
            return;
        };
        drains.push(LocalDrain {
            center,
            direction: normal.map(|c| -c),
            radius_mm: self.options.borrow().drain_radius_mm,
        });
        self.launch(leaf, mesh, original, drains, &world);
    }

    fn launch(
        self: &Rc<Self>,
        leaf: NodeId,
        before: Arc<IndexedMesh>,
        original: Arc<IndexedMesh>,
        drains: Vec<LocalDrain>,
        world: &Transform3D,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.job.borrow().is_some() || ui.get_loading() {
            return;
        }
        let options = *self.options.borrow();
        let request = HollowRequest {
            kind: HollowKind::Apply,
            grid: GridRequest {
                voxel_size_mm: options.voxel_mm,
                ..GridRequest::default()
            },
            settings: options.settings(),
            drains: drains.iter().map(|d| d.to_world(world)).collect(),
            prepared: self.prepared.borrow().clone(),
            ..HollowRequest::default()
        };
        match start_hollow_mesh(original.clone(), world, request) {
            Ok(handle) => {
                *self.job.borrow_mut() = Some(Job {
                    handle,
                    leaf,
                    started_on: self.document.scene_revision(),
                    before,
                    original,
                    drains,
                });
                ui.set_hollow_running(true);
                ui.set_hollow_progress(0.0);
                ui.set_hollow_phase(slint::SharedString::default());
                self.start_poll();
            }
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Hollowing failed"),
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
                    let slot = run.job.borrow();
                    let Some(job) = slot.as_ref() else {
                        stop();
                        return;
                    };
                    let progress = job.handle.progress();
                    ui.set_hollow_progress(progress.fraction());
                    ui.set_hollow_phase(progress.phase.into());
                    job.handle.try_poll()
                };
                if let Some(outcome) = finished {
                    stop();
                    let job = run.job.borrow_mut().take();
                    ui.set_hollow_running(false);
                    if let Some(job) = job {
                        run.finish(job, outcome);
                    }
                }
            },
        );
    }

    fn finish(&self, job: Job, outcome: Result<HollowOutcome, JobError>) {
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JobError::Cancelled) => return,
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Hollowing failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
                return;
            }
        };
        *self.prepared.borrow_mut() = Some(outcome.prepared.clone());
        let still_there = self
            .document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .leaf_mesh_arc(job.leaf)
                    .is_some_and(|m| Arc::ptr_eq(m, &job.before))
            })
            .unwrap_or(false);
        if !still_there || self.document.slice_geometry_changed(job.started_on) {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("HollowRun", "Hollow discarded"),
                message: ctr!(
                    "HollowRun",
                    "The scene changed while the model was being hollowed; nothing was changed."
                ),
                details: Default::default(),
            });
            return;
        }
        let HollowResult::Apply { mesh, drains } = outcome.result else {
            return;
        };
        let hollow = Arc::new(mesh);
        if !self.swap_mesh(job.leaf, job.before, hollow.clone()) {
            return;
        }
        let stats = &outcome.stats;
        let ml = |mm3: f64| mm3 / 1000.0;
        let source = ml(stats.source_volume_mm3);
        let saved = ml(stats.removed_volume_mm3.unwrap_or(0.0));
        let percent = if source > 0.0 {
            saved / source * 100.0
        } else {
            0.0
        };
        let blind = drains.iter().filter(|d| !d.reaches_cavity).count();
        let mut summary = format!(
            "Saves {saved:.1} of {source:.1} ml ({percent:.0} %). Wall {:.1} mm, voxel {:.2} mm.",
            stats.wall_thickness_mm, stats.grid.voxel_mm,
        );
        if !drains.is_empty() {
            summary.push_str(&format!(" {} drain(s)", drains.len()));
            if blind > 0 {
                summary.push_str(&format!(", {blind} not reaching the cavity"));
            }
            summary.push('.');
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_hollow_last_run(summary.into());
        }
        self.hollowed.borrow_mut().push(Hollowed {
            hollow,
            original: job.original,
            drains: job.drains,
        });
        self.refresh_ui();
    }

    /// Replaces the leaf's mesh as one undoable edit.
    fn swap_mesh(&self, leaf: NodeId, before: Arc<IndexedMesh>, after: Arc<IndexedMesh>) -> bool {
        let edit = self
            .document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                // The leaf must still show `before`: never swap out a mesh
                // that changed under the job.
                scene
                    .leaf_mesh_arc(leaf)
                    .is_some_and(|m| Arc::ptr_eq(m, &before))
                    .then(|| leaf_mesh_swap(scene, leaf, after))
                    .flatten()
            })
            .flatten();
        let Some(edit) = edit else {
            return false;
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(_) => {
                self.document.push_edit(edit);
                true
            }
            Err(err) => {
                let details = format!("{err:?}");
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("HollowRun", "Hollowing failed"),
                    context: None,
                    message: ctr!(
                        "HollowRun",
                        "The scene rejected the new mesh; nothing was changed."
                    ),
                    details: details.as_str().into(),
                });
                false
            }
        }
    }
}

impl HollowRun {
    /// Takes the previews down, if they are what the viewport shows.
    fn hide_preview(&self) {
        let mut preview = self.preview.borrow_mut();
        if let Some((_, shown)) = preview.shown.take()
            && self.ctrl.preview_overlay_is(&shown)
        {
            self.ctrl.set_preview_overlay(None);
        }
    }

    /// Brings the previews in line with the tab: the selected model's cavity
    /// for the current settings (computing it when they have rested), its
    /// drains, and the drain under the cursor.
    fn tick_preview(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_workspace_section() != WorkspaceSection::Hollow || ui.get_loading() {
            self.hide_preview();
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            self.hide_preview();
            return;
        };
        if self.model_state(&mesh) == 2 {
            self.hide_preview();
            return;
        }
        let (original, drains) = self
            .entry_for(&mesh)
            .unwrap_or_else(|| (mesh.clone(), Vec::new()));
        let options = *self.options.borrow();
        let key = PreviewKey {
            leaf,
            mesh: Arc::as_ptr(&original) as usize,
            options,
        };
        let show_cavity = ui.get_hollow_show_cavity();
        if show_cavity {
            self.advance_cavity_job(key, &original, &world);
        }

        let cavity = {
            let preview = self.preview.borrow();
            preview
                .ready
                .as_ref()
                .filter(|(k, _)| show_cavity && *k == key)
                .map(|(_, cavity)| cavity.clone())
        };
        let hover = self
            .hover
            .get()
            .filter(|_| ui.get_hollow_drain_tool() && self.model_state(&mesh) == 1);
        let shown_key = ShownKey {
            cavity: cavity.as_ref().map(|c| Arc::as_ptr(c) as usize),
            world: matrix_of(&world),
            drains: drains.clone(),
            hover,
            radius: options.drain_radius_mm,
            wall: options.wall_mm,
        };
        {
            let preview = self.preview.borrow();
            if let Some((k, shown)) = &preview.shown
                && *k == shown_key
                && self.ctrl.preview_overlay_is(shown)
            {
                return;
            }
        }
        let mut pieces = Vec::new();
        if let Some(cavity) = &cavity {
            let mut piece = crate::df_support_run::overlay_of(cavity, OverlayTint::Cavity);
            for p in &mut piece.positions {
                *p = world.apply_to_vertex(Vertex::new(*p)).coords();
            }
            pieces.push(piece);
        }
        let depth = options.wall_mm + 1.0;
        let mut existing = (Vec::new(), Vec::new());
        for drain in &drains {
            let w = drain.to_world(&world);
            cylinder(w.center, w.direction, w.radius_mm, depth, &mut existing);
        }
        if !existing.1.is_empty() {
            pieces.push(OverlayMesh {
                positions: existing.0,
                indices: existing.1,
                tint: OverlayTint::Drain,
            });
        }
        if let Some(point) = hover
            && let Ok(inverse) = world.inverse()
        {
            let local = inverse.apply_to_vertex(Vertex::new(point)).coords();
            if let Some((center, normal)) = closest_surface_point(&original, local) {
                let w = LocalDrain {
                    center,
                    direction: normal.map(|c| -c),
                    radius_mm: options.drain_radius_mm,
                }
                .to_world(&world);
                let mut next = (Vec::new(), Vec::new());
                cylinder(w.center, w.direction, w.radius_mm, depth, &mut next);
                pieces.push(OverlayMesh {
                    positions: next.0,
                    indices: next.1,
                    tint: OverlayTint::Valid,
                });
            }
        }
        let overlay: Arc<[OverlayMesh]> = pieces.into();
        self.ctrl.set_preview_overlay(Some(overlay.clone()));
        self.preview.borrow_mut().shown = Some((shown_key, overlay));
    }

    /// Polls the cavity job, and starts one for `key` once the settings have
    /// rested and nothing else is hollowing.
    fn advance_cavity_job(
        &self,
        key: PreviewKey,
        original: &Arc<IndexedMesh>,
        world: &Transform3D,
    ) {
        let mut preview = self.preview.borrow_mut();
        if let Some((handle, job_key)) = &preview.job {
            if *job_key != key {
                handle.cancel();
            }
            match handle.try_poll() {
                None if *job_key == key => return,
                None => {}
                Some(Ok(outcome)) => {
                    let job_key = *job_key;
                    *self.prepared.borrow_mut() = Some(outcome.prepared.clone());
                    if let HollowResult::Preview { cavity, infill, .. } = outcome.result {
                        let mut cavity = cavity;
                        if let Some(infill) = infill {
                            append_mesh(&mut cavity, &infill);
                        }
                        preview.ready = Some((job_key, Arc::new(cavity)));
                    }
                }
                Some(Err(_)) => {}
            }
            preview.job = None;
        }
        let ready = preview.ready.as_ref().is_some_and(|(k, _)| *k == key);
        if ready
            || self.job.borrow().is_some()
            || self.options_changed.get().elapsed() < PREVIEW_SETTLE
        {
            return;
        }
        let request = HollowRequest {
            kind: HollowKind::Preview,
            grid: GridRequest {
                voxel_size_mm: key.options.voxel_mm,
                ..GridRequest::default()
            },
            settings: key.options.settings(),
            prepared: self.prepared.borrow().clone(),
            ..HollowRequest::default()
        };
        if let Ok(handle) = start_hollow_mesh(original.clone(), world, request) {
            preview.job = Some((handle, key));
        }
    }
}

/// The 4x4 of a transform, to tell poses apart.
fn matrix_of(world: &Transform3D) -> [[f64; 4]; 4] {
    let at = |p: [f32; 3]| {
        world
            .apply_to_vertex(Vertex::new(p))
            .coords()
            .map(f64::from)
    };
    let (o, x, y, z) = (
        at([0.0; 3]),
        at([1.0, 0.0, 0.0]),
        at([0.0, 1.0, 0.0]),
        at([0.0, 0.0, 1.0]),
    );
    [
        [x[0] - o[0], x[1] - o[1], x[2] - o[2], 0.0],
        [y[0] - o[0], y[1] - o[1], y[2] - o[2], 0.0],
        [z[0] - o[0], z[1] - o[1], z[2] - o[2], 0.0],
        [o[0], o[1], o[2], 1.0],
    ]
}

/// `extra`'s triangles added to `mesh`.
fn append_mesh(mesh: &mut IndexedMesh, extra: &IndexedMesh) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.extend_from_slice(&extra.vertices);
    mesh.faces.extend(extra.faces.iter().map(|f| {
        sc_mesh_core::IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + base))
    }));
}

/// A closed cylinder (a drain as drawn) from just outside `center` to
/// `depth` along `direction`, added to `out`.
fn cylinder(
    center: [f32; 3],
    direction: [f32; 3],
    radius: f32,
    depth: f32,
    out: &mut (Vec<[f32; 3]>, Vec<u32>),
) {
    let d = normalized(direction);
    let helper = if d[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = normalized(cross(d, helper));
    let v = cross(d, u);
    let start: [f32; 3] = std::array::from_fn(|i| center[i] - d[i] * 0.5);
    let end: [f32; 3] = std::array::from_fn(|i| center[i] + d[i] * depth);
    let (positions, indices) = out;
    let base = positions.len() as u32;
    for k in 0..DRAIN_SIDES {
        let a = k as f32 / DRAIN_SIDES as f32 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let off: [f32; 3] = std::array::from_fn(|i| (u[i] * c + v[i] * s) * radius);
        positions.push(std::array::from_fn(|i| start[i] + off[i]));
        positions.push(std::array::from_fn(|i| end[i] + off[i]));
    }
    positions.push(start);
    positions.push(end);
    let (cs, ce) = (base + 2 * DRAIN_SIDES, base + 2 * DRAIN_SIDES + 1);
    for k in 0..DRAIN_SIDES {
        let (a0, a1) = (base + 2 * k, base + 2 * k + 1);
        let n = (k + 1) % DRAIN_SIDES;
        let (b0, b1) = (base + 2 * n, base + 2 * n + 1);
        indices.extend_from_slice(&[a0, b0, b1, a0, b1, a1, cs, b0, a0, ce, a1, b1]);
    }
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 {
        v.map(|c| c / len)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The point of `mesh` closest to `p` and the outward normal of its face.
fn closest_surface_point(mesh: &IndexedMesh, p: [f32; 3]) -> Option<([f32; 3], [f32; 3])> {
    let vs = &mesh.vertices;
    let mut best: Option<(f32, [f32; 3], [f32; 3])> = None;
    for f in &mesh.faces {
        let [a, b, c] = f.vertices.map(|i| vs[i.get() as usize].coords());
        let q = closest_on_triangle(p, a, b, c);
        let d = sub(q, p);
        let dist = dot(d, d);
        if best.is_none_or(|(bd, _, _)| dist < bd) {
            best = Some((dist, q, cross(sub(b, a), sub(c, a))));
        }
    }
    best.map(|(_, q, n)| (q, normalized(n)))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision
/// Detection, 5.1.5).
fn closest_on_triangle(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let at = |s: f32, u: [f32; 3], t: f32, w: [f32; 3]| {
        std::array::from_fn(|i| a[i] + s * u[i] + t * w[i])
    };
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return at(v, ab, 0.0, ac);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return at(0.0, ab, w, ac);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return std::array::from_fn(|i| b[i] + w * (c[i] - b[i]));
    }
    let denom = 1.0 / (va + vb + vc);
    at(vb * denom, ab, vc * denom, ac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closest_point_is_on_the_face_or_its_edges() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
        assert_eq!(
            closest_on_triangle([0.5, 0.5, 3.0], a, b, c),
            [0.5, 0.5, 0.0]
        );
        assert_eq!(closest_on_triangle([-1.0, -1.0, 0.0], a, b, c), a);
        assert_eq!(
            closest_on_triangle([1.0, -2.0, 0.0], a, b, c),
            [1.0, 0.0, 0.0]
        );
        assert_eq!(
            closest_on_triangle([3.0, 3.0, 0.0], a, b, c),
            [1.0, 1.0, 0.0]
        );
    }

    #[test]
    fn a_local_drain_follows_the_model_pose() {
        let drain = LocalDrain {
            center: [1.0, 0.0, 0.0],
            direction: [-1.0, 0.0, 0.0],
            radius_mm: 1.5,
        };
        let world =
            Transform3D::Affine(sc_mesh_core::transform::AffineTransform3D::from_rotation_z(
                std::f64::consts::FRAC_PI_2,
            ));
        let w = drain.to_world(&world);
        assert!((w.center[1] - 1.0).abs() < 1e-5 && w.center[0].abs() < 1e-5);
        assert!((w.direction[1] + 1.0).abs() < 1e-5);
        assert_eq!(w.radius_mm, 1.5);
    }

    #[test]
    fn options_build_the_engine_settings() {
        let mut o = HollowOptions::default();
        assert_eq!(o.settings().fill, HollowFill::Cavity);
        o.fill = 2;
        assert!(matches!(
            o.settings().fill,
            HollowFill::Infill {
                pattern: InfillMode::Pillar,
                ..
            }
        ));
    }
}
