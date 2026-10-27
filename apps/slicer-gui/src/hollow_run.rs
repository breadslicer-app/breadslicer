/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Hollow tab (BS-8): hollows the selected model with the DragonFruit
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
//!
//! The Review card (#90) asks which hollow made here holds a resin trap
//! ([`HollowRun::hollow_at`]) and opens the drain tool on that model (#158).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use sc_geom::df_hollow::hollow_df::{GridRequest, HollowFill, HollowSettings, InfillMode};
use sc_geom::df_hollow::{
    HollowKind, HollowOutcome, HollowRequest, HollowResult, PreparedHollow, WorldDrain,
    start_hollow_mesh,
};
use sc_geom::{JobError, JobHandle};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_scene::NodeId;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};
use crate::scene_document::{
    SceneDocument, SceneEvent, SceneRevision, SceneSubscription, leaf_mesh_swap,
};
use crate::viewport::ViewportController;
use crate::{MainWindow, WorkspaceSection};

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How often the previews look at the tab, the selection and the settings.
const PREVIEW_TICK: Duration = Duration::from_millis(40);
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

/// Where a vertical line down from `at` (world mm) first leaves the solid of
/// `mesh` posed by `world`: below a cavity's floor point, the underside of
/// that floor. The faces are read in the mesh's own frame, where the line is
/// `at + t * down` mapped back; `t` is then the world distance, since the
/// map is affine. A face is left, not entered, when its outward normal (the
/// winding's) points along the line.
fn floor_underside(mesh: &IndexedMesh, world: &Transform3D, at: [f32; 3]) -> Option<[f32; 3]> {
    let inverse = world.inverse().ok()?;
    let local = |p: [f32; 3]| inverse.apply_to_vertex(Vertex::new(p)).coords();
    let origin = local(at);
    let below = local([at[0], at[1], at[2] - 1.0]);
    let down: [f32; 3] = std::array::from_fn(|i| below[i] - origin[i]);
    let vertices = mesh.vertices();
    let mut nearest: Option<f32> = None;
    for face in mesh.faces() {
        let corner = |k: usize| {
            vertices
                .get(usize::from(face.vertices[k]))
                .map(|v| v.coords())
        };
        let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
            continue;
        };
        let e1: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
        let e2: [f32; 3] = std::array::from_fn(|i| c[i] - a[i]);
        if dot(cross(e1, e2), down) <= 0.0 {
            continue;
        }
        // The determinant is minus the normal's projection on the line, so a
        // face the line leaves never gives 0, save by underflow.
        let h = cross(down, e2);
        let det = dot(e1, h);
        if det == 0.0 {
            continue;
        }
        let s: [f32; 3] = std::array::from_fn(|i| origin[i] - a[i]);
        let u = dot(s, h) / det;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(down, q) / det;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = dot(e2, q) / det;
        if t > 1e-4 && nearest.is_none_or(|best| t < best) {
            nearest = Some(t);
        }
    }
    nearest.map(|t| [at[0], at[1], at[2] - t])
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
    /// The drain a click drills: at the world `point` hit, into the model
    /// against the surface `normal` there, in the leaf's local frame.
    fn at(point: [f32; 3], normal: [f32; 3], radius_mm: f32, world: &Transform3D) -> Option<Self> {
        let inverse = world.inverse().ok()?;
        let local = |p: [f32; 3]| inverse.apply_to_vertex(Vertex::new(p)).coords();
        let center = local(point);
        let inside = local(std::array::from_fn(|i| point[i] - normal[i]));
        Some(LocalDrain {
            center,
            direction: normalized(std::array::from_fn(|i| inside[i] - center[i])),
            radius_mm,
        })
    }

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
    /// Where the cursor rests on the model with the drain tool armed, and
    /// the surface normal there (world).
    hover: Cell<Option<([f32; 3], [f32; 3])>>,
    preview_timer: slint::Timer,
    /// Drains placed on a model not hollowed yet, by its mesh: drilled when
    /// it is hollowed.
    pending: RefCell<Vec<(usize, Vec<LocalDrain>)>>,
    /// The local bounding box of the last mesh asked about, by identity, for
    /// the recommended wall (a scan of every vertex once per mesh).
    bounds: RefCell<Option<MeshBounds>>,
}

/// A mesh (by identity) and its local bounding box.
type MeshBounds = (usize, [f32; 3], [f32; 3]);

/// The wall a model of this size wants: 1.5 mm plus 1 % of its longest side,
/// between 1.5 and 4 mm, to 0.1 mm. A 50 mm miniature gets the usual 2 mm, a
/// 150 mm figure 3 mm, a large piece up to 4 mm for its own weight.
fn recommended_wall_mm(longest_side_mm: f32) -> f32 {
    ((1.5 + 0.01 * longest_side_mm).clamp(1.5, 4.0) * 10.0).round() / 10.0
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
/// leaf's local frame), the overlay pieces drawn from them (kept while their
/// inputs hold, so the cursor moving uploads only the drain under it) and the
/// overlay last shown.
#[derive(Default)]
struct CavityPreview {
    job: Option<(JobHandle<HollowOutcome>, PreviewKey)>,
    ready: Option<(PreviewKey, Arc<IndexedMesh>)>,
    cavity_piece: Option<(CavityPieceKey, Arc<OverlayMesh>)>,
    drains_piece: Option<(DrainsPieceKey, Arc<OverlayMesh>)>,
    /// What the shown overlay was drawn from, and the overlay.
    shown: Option<(ShownKey, Overlay)>,
}

/// A drawn cavity: which cavity mesh, at which pose.
type CavityPieceKey = (usize, [[f64; 4]; 4]);
/// Drawn drains: which, at which pose, how deep (bits of the depth).
type DrainsPieceKey = (Vec<LocalDrain>, [[f64; 4]; 4], u32);

/// Everything the overlay is drawn from.
#[derive(Debug, Clone, PartialEq)]
struct ShownKey {
    cavity: Option<usize>,
    drains: Option<usize>,
    hover: Option<([f32; 3], [f32; 3])>,
    world: [[f64; 4]; 4],
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
                hover: Cell::new(None),
                preview_timer: slint::Timer::default(),
                pending: RefCell::new(Vec::new()),
                bounds: RefCell::new(None),
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
            ui.on_hollow_drain_hover(move |x, y, z, nx, ny, nz, hit| {
                if let Some(run) = weak.upgrade() {
                    run.hover.set(hit.then_some(([x, y, z], [nx, ny, nz])));
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
            ui.on_hollow_drain_clicked(move |x, y, z, nx, ny, nz| {
                run.on_drain_click([x, y, z], [nx, ny, nz]);
            });
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
        if sc_geom::df_supports::is_support_mesh(mesh) {
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

    /// What a model is hollowed from and its drains: a hollow's original and
    /// drilled drains, or a solid model and the drains placed on it so far.
    fn source_for(&self, mesh: &Arc<IndexedMesh>) -> (Arc<IndexedMesh>, Vec<LocalDrain>) {
        self.entry_for(mesh).unwrap_or_else(|| {
            let ptr = Arc::as_ptr(mesh) as usize;
            let drains = self
                .pending
                .borrow()
                .iter()
                .find(|(m, _)| *m == ptr)
                .map(|(_, d)| d.clone())
                .unwrap_or_default();
            (mesh.clone(), drains)
        })
    }

    fn set_pending(&self, mesh: &Arc<IndexedMesh>, drains: Vec<LocalDrain>) {
        let ptr = Arc::as_ptr(mesh) as usize;
        let mut pending = self.pending.borrow_mut();
        pending.retain(|(m, _)| *m != ptr);
        if !drains.is_empty() {
            pending.push((ptr, drains));
        }
    }

    /// The selected model's longest side in the world (its bounding box at
    /// the current pose).
    fn longest_side_mm(&self, mesh: &Arc<IndexedMesh>, world: &Transform3D) -> f32 {
        let ptr = Arc::as_ptr(mesh) as usize;
        let (lo, hi) = {
            let mut cached = self.bounds.borrow_mut();
            match *cached {
                Some((p, lo, hi)) if p == ptr => (lo, hi),
                _ => {
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for v in &mesh.vertices {
                        for k in 0..3 {
                            lo[k] = lo[k].min(v.as_array()[k]);
                            hi[k] = hi[k].max(v.as_array()[k]);
                        }
                    }
                    *cached = Some((ptr, lo, hi));
                    (lo, hi)
                }
            }
        };
        let corners = (0..8).map(|c| {
            let p = std::array::from_fn(|k| if c >> k & 1 == 0 { lo[k] } else { hi[k] });
            world.apply_to_vertex(Vertex::new(p)).coords()
        });
        let (mut wlo, mut whi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in corners {
            for k in 0..3 {
                wlo[k] = wlo[k].min(p[k]);
                whi[k] = whi[k].max(p[k]);
            }
        }
        (0..3).map(|k| whi[k] - wlo[k]).fold(0.0, f32::max)
    }

    fn refresh_ui(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let (state, drains) = match self.selected() {
            Some((_, mesh, world)) => {
                let (original, drains) = self.source_for(&mesh);
                ui.set_hollow_recommended_wall(recommended_wall_mm(
                    self.longest_side_mm(&original, &world),
                ));
                (self.model_state(&mesh), drains.len())
            }
            None => (2, 0),
        };
        ui.set_hollow_model_state(state);
        ui.set_hollow_drain_count(drains as i32);
        if state == 2 {
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
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
        self.tick_preview();
    }

    fn on_apply(self: &Rc<Self>) {
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        // Hollowing a hollow again starts from its original, keeping drains;
        // a solid model takes the drains placed on it.
        let (original, drains) = self.source_for(&mesh);
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
            self.set_pending(&mesh, Vec::new());
            self.refresh_ui();
            return;
        };
        if !drains.is_empty() {
            self.launch(leaf, mesh, original, Vec::new(), &world);
        }
    }

    /// A click with the drain tool at a world point and surface normal on
    /// the selected model (the viewport selects the model it hit first). A
    /// hollow is hollowed again with the new drain; a solid model keeps it
    /// for when it is hollowed.
    fn on_drain_click(self: &Rc<Self>, point: [f32; 3], normal: [f32; 3]) {
        if self.job.borrow().is_some() {
            return;
        }
        let Some((leaf, mesh, world)) = self.selected() else {
            return;
        };
        if self.model_state(&mesh) == 2 {
            return;
        }
        let Some(drain) =
            LocalDrain::at(point, normal, self.options.borrow().drain_radius_mm, &world)
        else {
            return;
        };
        let (original, mut drains) = self.source_for(&mesh);
        drains.push(drain);
        if self.entry_for(&mesh).is_some() {
            self.launch(leaf, mesh, original, drains, &world);
        } else {
            self.set_pending(&mesh, drains);
            self.refresh_ui();
            self.tick_preview();
        }
    }

    /// The drain the review suggests for a cavity whose floor is at `at`
    /// (world mm): on a hollow made here, on the underside of that floor
    /// straight below `at`, pointing up into the cavity. Of several hollows,
    /// the one whose underside is nearest below `at`.
    fn drain_target(
        &self,
        at: [f32; 3],
    ) -> Option<(NodeId, Arc<IndexedMesh>, Transform3D, LocalDrain)> {
        let radius_mm = self.options.borrow().drain_radius_mm;
        let hollows: Vec<Arc<IndexedMesh>> = self
            .hollowed
            .borrow()
            .iter()
            .map(|h| h.hollow.clone())
            .collect();
        self.document
            .with_scene(|spatial| {
                let scene = spatial.graph();
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter_map(|leaf| {
                        let mesh = scene.leaf_mesh_arc(leaf)?;
                        if !hollows.iter().any(|h| Arc::ptr_eq(h, mesh)) {
                            return None;
                        }
                        let world = scene.world_transform_of(leaf).ok()?;
                        let exit = floor_underside(mesh, &world, at)?;
                        Some((leaf, mesh.clone(), world, exit))
                    })
                    .max_by(|a, b| a.3[2].total_cmp(&b.3[2]))
            })
            .flatten()
            .and_then(|(leaf, mesh, world, exit)| {
                let drain = LocalDrain::at(exit, [0.0, 0.0, -1.0], radius_mm, &world)?;
                Some((leaf, mesh, world, drain))
            })
    }

    /// The model hollowed here whose hollow holds the cavity at `at` (world
    /// mm), and how many drain holes that hollow has; `None` when no model
    /// hollowed here holds it.
    pub(crate) fn hollow_at(&self, at: [f32; 3]) -> Option<(NodeId, usize)> {
        let (leaf, mesh, _, _) = self.drain_target(at)?;
        Some((leaf, self.source_for(&mesh).1.len()))
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
        crate::breadcrumbs::note(&format!("hollow: finished ok={}", outcome.is_ok()));
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
        // The drains placed before are drilled now: they live on the hollow.
        let original_ptr = Arc::as_ptr(&job.original) as usize;
        self.pending
            .borrow_mut()
            .retain(|(m, _)| *m != original_ptr);
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
    /// for the current settings (computing it when they have rested; the last
    /// one stays up meanwhile), its drains, and the drain under the cursor.
    fn tick_preview(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_hollow_busy(
            self.job.borrow().is_some()
                || self
                    .preview
                    .try_borrow()
                    .is_ok_and(|preview| preview.job.is_some()),
        );
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
        let (original, drains) = self.source_for(&mesh);
        let options = *self.options.borrow();
        let key = PreviewKey {
            leaf,
            mesh: Arc::as_ptr(&original) as usize,
            // The drain radius does not change the cavity.
            options: HollowOptions {
                drain_radius_mm: 0.0,
                ..options
            },
        };
        let show_cavity = ui.get_hollow_show_cavity();
        if show_cavity {
            self.advance_cavity_job(key, &original, &world);
        }
        let matrix = matrix_of(&world);
        let depth = options.wall_mm + 1.0;
        let hover = self.hover.get().filter(|_| ui.get_hollow_drain_tool());

        let mut preview = self.preview.borrow_mut();
        // The cavity of this model, current or the last one while the next
        // is computed.
        let cavity = preview
            .ready
            .as_ref()
            .filter(|(k, _)| show_cavity && k.leaf == key.leaf && k.mesh == key.mesh)
            .map(|(_, cavity)| cavity.clone());
        let cavity_piece = cavity.map(|cavity| {
            let id = (Arc::as_ptr(&cavity) as usize, matrix);
            match &preview.cavity_piece {
                Some((k, piece)) if *k == id => piece.clone(),
                _ => {
                    let mut piece = crate::df_support_run::overlay_of(&cavity, OverlayTint::Cavity);
                    for p in &mut piece.positions {
                        *p = world.apply_to_vertex(Vertex::new(*p)).coords();
                    }
                    let piece = Arc::new(piece);
                    preview.cavity_piece = Some((id, piece.clone()));
                    piece
                }
            }
        });
        let drains_piece = (!drains.is_empty()).then(|| {
            let id = (drains.clone(), matrix, depth.to_bits());
            match &preview.drains_piece {
                Some((k, piece)) if *k == id => piece.clone(),
                _ => {
                    let mut mesh = (Vec::new(), Vec::new());
                    for drain in &drains {
                        let w = drain.to_world(&world);
                        cylinder(w.center, w.direction, w.radius_mm, depth, &mut mesh);
                    }
                    let piece = Arc::new(OverlayMesh {
                        positions: mesh.0,
                        indices: mesh.1,
                        tint: OverlayTint::Drain,
                    });
                    preview.drains_piece = Some((id, piece.clone()));
                    piece
                }
            }
        });
        let shown_key = ShownKey {
            cavity: cavity_piece.as_ref().map(|p| Arc::as_ptr(p) as usize),
            drains: drains_piece.as_ref().map(|p| Arc::as_ptr(p) as usize),
            hover,
            world: matrix,
            radius: options.drain_radius_mm,
            wall: options.wall_mm,
        };
        if let Some((k, shown)) = &preview.shown
            && *k == shown_key
            && self.ctrl.preview_overlay_is(shown)
        {
            return;
        }
        let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
        pieces.extend(cavity_piece);
        pieces.extend(drains_piece);
        if let Some((point, normal)) = hover {
            // Straight from the surface under the cursor, where the click
            // would drill: no search, the viewport's hit says it.
            let direction = normal.map(|c| -c);
            let mut next = (Vec::new(), Vec::new());
            cylinder(point, direction, options.drain_radius_mm, depth, &mut next);
            pieces.push(Arc::new(OverlayMesh {
                positions: next.0,
                indices: next.1,
                tint: OverlayTint::Valid,
            }));
        }
        crate::breadcrumbs::note(&format!(
            "hollow: preview cavity={} drains={} hover={}",
            shown_key.cavity.is_some(),
            shown_key.drains.is_some(),
            shown_key.hover.is_some()
        ));
        let overlay: Overlay = pieces.into();
        self.ctrl.set_preview_overlay(Some(overlay.clone()));
        preview.shown = Some((shown_key, overlay));
    }

    /// Polls the cavity job, and starts one for `key` when none runs. A job
    /// for older settings of the same model is let finish (its cavity shows
    /// meanwhile), so dragging the wall updates the preview continuously at
    /// the engine's pace; one for another model is cancelled.
    fn advance_cavity_job(
        &self,
        key: PreviewKey,
        original: &Arc<IndexedMesh>,
        world: &Transform3D,
    ) {
        let mut preview = self.preview.borrow_mut();
        if let Some((handle, job_key)) = &preview.job {
            let same_model = job_key.leaf == key.leaf && job_key.mesh == key.mesh;
            if !same_model {
                handle.cancel();
            }
            match handle.try_poll() {
                None if same_model => return,
                None => {}
                Some(Ok(outcome)) => {
                    crate::breadcrumbs::note("hollow: cavity preview ready");
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
        if ready || self.job.borrow().is_some() {
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
            crate::breadcrumbs::note("hollow: cavity preview started");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recommended_wall_grows_with_the_model() {
        assert_eq!(recommended_wall_mm(20.0), 1.7);
        assert_eq!(recommended_wall_mm(50.0), 2.0);
        assert_eq!(recommended_wall_mm(150.0), 3.0);
        assert_eq!(recommended_wall_mm(400.0), 4.0);
        assert_eq!(recommended_wall_mm(0.0), 1.5);
    }

    #[test]
    fn a_drain_from_a_hit_points_into_the_model() {
        // Hit on the top face (normal +Z) of a model moved by (10, 0, 0).
        let world = Transform3D::Affine(
            sc_mesh_core::transform::AffineTransform3D::from_translation([10.0, 0.0, 0.0]),
        );
        let drain = LocalDrain::at([10.0, 0.0, 5.0], [0.0, 0.0, 1.0], 1.5, &world).unwrap();
        assert_eq!(drain.center, [0.0, 0.0, 5.0]);
        assert_eq!(drain.direction, [0.0, 0.0, -1.0]);
    }

    /// A 10 mm box standing on the plate, hollowed to a 6 mm cavity: walls
    /// of 2 mm, the cavity floor at z = 2.
    fn hollow_box() -> IndexedMesh {
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let scaled = |k: f32| {
            cube.vertices
                .iter()
                .map(move |v| Vertex::new(v.coords().map(|c| c * k)))
        };
        let vertices: Vec<Vertex> = scaled(10.0).chain(scaled(6.0)).collect();
        let mut faces = cube.faces.clone();
        faces.extend(cube.faces.iter().map(|f| {
            let [a, b, c] = f.vertices.map(|v| usize::from(v) as u32 + 8);
            sc_mesh_core::IndexedTriangle::from_raw_indices([a, c, b])
        }));
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn standing(transform: sc_mesh_core::transform::AffineTransform3D) -> Transform3D {
        Transform3D::Affine(
            sc_mesh_core::transform::AffineTransform3D::from_translation([0.0, 0.0, 5.0])
                .compose(&transform),
        )
    }

    #[test]
    fn a_drain_under_a_cavity_floor_leaves_through_the_underside() {
        let mesh = hollow_box();
        let world = standing(sc_mesh_core::transform::AffineTransform3D::identity());
        // The review's floor point is on the cavity floor, give or take half
        // a layer: either way the line leaves the solid at the box's bottom.
        for z in [2.05, 2.0, 1.97] {
            let exit = floor_underside(&mesh, &world, [0.3, -0.2, z]).expect("under the floor");
            assert!((exit[2] - 0.0).abs() < 1e-4, "{z}: {exit:?}");
            assert_eq!([exit[0], exit[1]], [0.3, -0.2]);
        }
        // Beside the box, nothing.
        assert_eq!(floor_underside(&mesh, &world, [8.0, 0.0, 2.05]), None);
    }

    #[test]
    fn a_mirrored_or_scaled_hollow_keeps_its_underside() {
        let mesh = hollow_box();
        let mirrored = standing(
            sc_mesh_core::transform::AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]),
        );
        let exit = floor_underside(&mesh, &mirrored, [1.0, 1.0, 2.05]).expect("mirrored");
        assert!(exit[2].abs() < 1e-4, "{exit:?}");
        let scaled = Transform3D::Affine(
            sc_mesh_core::transform::AffineTransform3D::from_translation([0.0, 0.0, 10.0])
                .compose(&sc_mesh_core::transform::AffineTransform3D::from_uniform_scale(2.0)),
        );
        // Twice the size: the cavity floor is at z = 4 and the underside at 0.
        let exit = floor_underside(&mesh, &scaled, [1.0, 1.0, 4.05]).expect("scaled");
        assert!(exit[2].abs() < 1e-4, "{exit:?}");
    }

    #[test]
    fn a_drain_under_the_floor_points_up_into_the_cavity() {
        let world = standing(sc_mesh_core::transform::AffineTransform3D::identity());
        let drain = LocalDrain::at([0.3, -0.2, 0.0], [0.0, 0.0, -1.0], 1.5, &world).unwrap();
        assert_eq!(drain.center, [0.3, -0.2, -5.0]);
        assert_eq!(drain.direction, [0.0, 0.0, 1.0]);
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

    /// Dragging the wall slider changes the wall many times a second: the
    /// cavity shown must stay up throughout (the last one until the next is
    /// computed), never blink out.
    #[test]
    fn the_cavity_stays_shown_while_the_wall_changes() {
        use crate::selection::{LeafId, Selection};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        let shown = on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut cube = sc_mesh_fixtures::unit_cube_mesh();
            for v in &mut cube.vertices {
                *v = Vertex::new(v.as_array().map(|c| c * 30.0));
            }
            let mut graph = sc_scene::Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(
                        sc_mesh_core::transform::AffineTransform3D::from_translation([
                            50.0, 50.0, 15.0,
                        ]),
                    ),
                    sc_scene::node::LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, std::path::PathBuf::from("cube.stl"));
            document.set_selection(Selection::One(LeafId(leaf)));
            window.set_workspace_section(WorkspaceSection::Hollow);
            let run = HollowRun::install(&window, document, ctrl.dialogs(), ctrl.clone());
            let cavity_shown = |run: &HollowRun| {
                run.preview
                    .borrow()
                    .shown
                    .as_ref()
                    .is_some_and(|(_, overlay)| {
                        ctrl.preview_overlay_is(overlay)
                            && overlay.iter().any(|p| p.tint == OverlayTint::Cavity)
                    })
            };
            let waited = std::time::Instant::now();
            while !cavity_shown(&run) && waited.elapsed() < Duration::from_secs(20) {
                run.tick_preview();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(cavity_shown(&run), "a first cavity");
            (0..30)
                .map(|i| {
                    run.on_option(0, 2.0 + 0.05 * i as f32);
                    std::thread::sleep(Duration::from_millis(15));
                    run.tick_preview();
                    cavity_shown(&run)
                })
                .collect::<Vec<_>>()
        });
        assert!(shown.iter().all(|&s| s), "{shown:?}");
    }
}
