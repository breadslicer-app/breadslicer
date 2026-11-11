/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Cut tool (#56): a plane cut in the Transform tab.
//!
//! The tool (the rail's Cut button, or `K`) shows a plane through the
//! selected model: its height and its tilt are set on the card, and it can be
//! bounded to a rectangle on the plane (#56: cutting a miniature at the waist
//! while its long arms reach past it). Where the plane crosses the model
//! shows as the pieces of the cross-section ([`sc_cut`]): green the ones the
//! cut takes, grey the ones it leaves whole, amber the ones the rectangle's
//! edge crosses (the cut waits until the rectangle clears them, since cutting
//! part of a piece leaves a slit). A click on a piece takes it or leaves it.
//!
//! The section follows the card at once (a few milliseconds). Once the plane
//! settles, the cut itself is worked out on a worker, so the card says how
//! many pieces it leaves before anything changes, and says when a bounded
//! cut leaves one piece, marking in red what joins it. Cut then puts that
//! result in the scene: the model goes, its pieces come where it stood, each
//! closed, and its supports go with it, in one undo entry.
//!
//! A model that is not closed is repaired first, on a copy, as Slice asks:
//! the pieces come from the repaired model.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sc_cut::{Cut, CutError, CutModel, Fit, Plane, Rect, Section};
use sc_geom::df_supports::is_any_support_mesh;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::indexed_mesh::repair::repaired_copy;
use sc_mesh_core::transform::Transform3D;
use sc_scene::NodeId;
use sc_scene::node::SceneMetadata;
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};
use crate::scene_document::{EditNotApplied, SceneDocument, SceneEvent, SceneSubscription};
use crate::selection::{LeafId, Selection};
use crate::split_bodies::{leaf_name, removal_edit, split_edit};
use crate::support_persist::sets_of_model;
use crate::viewport::ViewportController;

/// How long the plane has to stay still before the cut is worked out.
const SETTLE: Duration = Duration::from_millis(180);

/// What the card sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CutSettings {
    /// The plane's height at the model's centre, world mm.
    pub(crate) height: f64,
    /// Its tilt about x and about y, degrees.
    pub(crate) tilt: [f64; 2],
    /// Whether the cut keeps to a rectangle on the plane.
    pub(crate) bounded: bool,
    /// The rectangle's width and depth, mm.
    pub(crate) size: [f64; 2],
    /// How far the rectangle's centre lies from the model's, along the
    /// plane's two directions, mm.
    pub(crate) offset: [f64; 2],
}

/// The model the tool works on, and where it stands.
#[derive(Clone)]
struct Target {
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
    /// The leaf's mesh to the world.
    world: Affine,
    /// Its box in the world.
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Target {
    fn selected(document: &SceneDocument) -> Option<Self> {
        let leaf = document.selection_primary_node_id()?;
        document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                let mesh = spatial.graph().leaf_mesh_arc(leaf)?.clone();
                if is_any_support_mesh(&mesh) || mesh.faces().is_empty() {
                    return None;
                }
                let world = Affine::of(&query.world_transform());
                let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
                for v in mesh.vertices() {
                    let p = world.apply(v.as_array().map(f64::from));
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                Some(Target {
                    leaf,
                    mesh,
                    world,
                    lo,
                    hi,
                })
            })
            .flatten()
    }

    fn center(&self) -> [f64; 3] {
        [0, 1, 2].map(|k| (self.lo[k] + self.hi[k]) / 2.0)
    }

    /// The same model in the same place.
    fn same(&self, other: &Self) -> bool {
        self.leaf == other.leaf && Arc::ptr_eq(&self.mesh, &other.mesh) && self.world == other.world
    }
}

/// An affine map in f64: `linear · p + translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine {
    linear: [[f64; 3]; 3],
    translation: [f64; 3],
}

impl Affine {
    fn of(t: &Transform3D) -> Self {
        match t {
            Transform3D::Identity => Self {
                linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                translation: [0.0; 3],
            },
            Transform3D::Affine(a) => Self {
                linear: *a.linear(),
                translation: *a.translation(),
            },
        }
    }

    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|r| {
            self.linear[r][0] * p[0]
                + self.linear[r][1] * p[1]
                + self.linear[r][2] * p[2]
                + self.translation[r]
        })
    }

    fn inverse(&self) -> Option<Self> {
        let m = self.linear;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let c = |r: usize, k: usize| {
            let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
            let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
            m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
        };
        // The inverse is the transposed cofactors over the determinant.
        let linear = [0, 1, 2].map(|r| [0, 1, 2].map(|k| c(k, r) / det));
        let t = self.translation;
        let translation =
            [0, 1, 2].map(|r| -(linear[r][0] * t[0] + linear[r][1] * t[1] + linear[r][2] * t[2]));
        Some(Self {
            linear,
            translation,
        })
    }

    /// Where a plane's normal goes: through the transposed linear part.
    fn normal_back(&self, n: [f64; 3]) -> [f64; 3] {
        [0, 1, 2]
            .map(|k| self.linear[0][k] * n[0] + self.linear[1][k] * n[1] + self.linear[2][k] * n[2])
    }
}

/// The plane and rectangle the settings make for a model, in the world.
struct Placed {
    plane: Plane,
    /// The plane's two directions in the world.
    u: [f64; 3],
    v: [f64; 3],
    /// The rectangle's centre in the world.
    center: [f64; 3],
    /// Half its width and depth: the model's whole reach when not bounded.
    half: [f64; 2],
}

fn placed(settings: &CutSettings, target: &Target) -> Option<Placed> {
    let c = target.center();
    let point = [c[0], c[1], settings.height];
    let (ax, ay) = (settings.tilt[0].to_radians(), settings.tilt[1].to_radians());
    // Tilted about x, then about y.
    let n = [ay.sin() * ax.cos(), -ax.sin(), ay.cos() * ax.cos()];
    let plane = Plane::through(point, n)?;
    let (u, v) = plane.frame();
    let reach = (0..3)
        .map(|k| target.hi[k] - target.lo[k])
        .fold(0.0f64, f64::max);
    let (center, half) = if settings.bounded {
        let [ou, ov] = settings.offset;
        (
            [0, 1, 2].map(|k| point[k] + u[k] * ou + v[k] * ov),
            [settings.size[0] / 2.0, settings.size[1] / 2.0],
        )
    } else {
        (point, [reach * 0.75 + 2.0; 2])
    };
    Some(Placed {
        plane,
        u,
        v,
        center,
        half,
    })
}

/// A model made ready to cut, for one mesh.
struct Ready {
    mesh: Arc<IndexedMesh>,
    model: CutModel,
    /// Whether the model had to be repaired first.
    repaired: bool,
}

/// What the worker hands back.
enum Worked {
    Ready(Result<Ready, String>),
    Trial(Result<Cut, CutError>),
}

type Slot = Arc<Mutex<Option<(u64, Worked)>>>;

/// The section on show, and what was chosen of it.
struct Shown {
    target: Target,
    settings: CutSettings,
    section: Arc<Section>,
    fits: Vec<Fit>,
    chosen: Vec<bool>,
    /// The cut worked out for exactly this, once it is.
    trial: Option<Arc<Cut>>,
    /// The worker job working it out, or 0.
    trial_job: u64,
}

pub(crate) struct CutRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    open: Cell<bool>,
    settings: Cell<CutSettings>,
    target: RefCell<Option<Target>>,
    ready: RefCell<Option<Arc<Ready>>>,
    /// The mesh being made ready.
    preparing: RefCell<Option<Arc<IndexedMesh>>>,
    shown: RefCell<Option<Shown>>,
    /// Pieces taken or left by a click, for the section on show.
    toggled: RefCell<HashMap<usize, bool>>,
    next_id: Cell<u64>,
    /// The job making a model ready, whose result is waited for.
    waiting: Cell<u64>,
    /// Apply was asked for before the trial cut was in.
    apply_when_in: Cell<bool>,
    slot: Slot,
    settle: slint::Timer,
    /// Clears the status bar's word on a cut.
    notice: slint::Timer,
    _scene_events: SceneSubscription,
}

impl CutRun {
    /// Wires the Cut tool; the caller keeps the driver alive for the
    /// window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new_cyclic(|me: &Weak<Self>| {
            let weak = me.clone();
            let scene_events = document.subscribe(move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::SelectionChanged
                        | SceneEvent::UndoStackChanged
                        | SceneEvent::Restructured
                ) {
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(run) = weak.upgrade() {
                            run.on_scene_changed();
                        }
                    });
                }
            });
            Self {
                ui: ui.as_weak(),
                document,
                dialogs,
                ctrl,
                open: Cell::new(false),
                settings: Cell::new(CutSettings {
                    height: 0.0,
                    tilt: [0.0; 2],
                    bounded: false,
                    size: [20.0; 2],
                    offset: [0.0; 2],
                }),
                target: RefCell::new(None),
                ready: RefCell::new(None),
                preparing: RefCell::new(None),
                shown: RefCell::new(None),
                toggled: RefCell::new(HashMap::new()),
                next_id: Cell::new(1),
                waiting: Cell::new(0),
                apply_when_in: Cell::new(false),
                slot: Arc::new(Mutex::new(None)),
                settle: slint::Timer::default(),
                notice: slint::Timer::default(),
                _scene_events: scene_events,
            }
        });
        macro_rules! wire {
            ($on:ident, |$run:ident $(, $arg:ident)*| $body:expr) => {{
                let $run = run.clone();
                ui.$on(move |$($arg),*| $body);
            }};
        }
        wire!(on_cut_toggle_requested, |r| r.toggle());
        wire!(on_cut_close_requested, |r| r.close());
        wire!(on_cut_option_changed, |r, code, value| r
            .on_option(code, value));
        wire!(on_cut_apply, |r| r.apply());
        wire!(on_cut_worked, |r| r.worked());
        let me = Rc::downgrade(&run);
        run.ctrl.set_cut_picker(Rc::new(move |origin, direction| {
            me.upgrade().is_some_and(|run| run.click(origin, direction))
        }));
        run
    }

    pub(crate) fn toggle(&self) {
        if self.open.get() {
            self.close();
        } else {
            self.open_tool();
        }
    }

    fn open_tool(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(target) = Target::selected(&self.document) else {
            return;
        };
        self.open.set(true);
        ui.set_cut_open(true);
        self.take_target(target, true);
    }

    /// Closes the tool; nothing in the scene changes.
    pub(crate) fn close(&self) {
        if !self.open.replace(false) {
            return;
        }
        self.settle.stop();
        self.waiting.set(0);
        self.apply_when_in.set(false);
        self.shown.borrow_mut().take();
        self.ctrl.set_preview_overlay(None);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_open(false);
            ui.set_cut_working(false);
            ui.set_cut_status("".into());
            ui.set_cut_warning("".into());
            ui.set_cut_can_cut(false);
        }
    }

    fn editable(&self) -> bool {
        self.ui.upgrade().is_some_and(|ui| {
            !ui.get_loading()
                && !ui.get_slicing_in_progress()
                && !ui.get_repairing_in_progress()
                && !ui.get_arranging()
        }) && self.ctrl.allows_layout_tools()
    }

    /// Works on `target`: a new model puts the plane through its middle.
    fn take_target(&self, target: Target, fresh: bool) {
        let new_model = self
            .target
            .borrow()
            .as_ref()
            .is_none_or(|t| t.leaf != target.leaf || !Arc::ptr_eq(&t.mesh, &target.mesh));
        if fresh || new_model {
            let mut s = self.settings.get();
            s.height = (target.lo[2] + target.hi[2]) / 2.0;
            s.size = [
                target.hi[0] - target.lo[0] + 2.0,
                target.hi[1] - target.lo[1] + 2.0,
            ];
            s.offset = [0.0; 2];
            self.settings.set(s);
        }
        *self.target.borrow_mut() = Some(target.clone());
        self.push_settings();
        let ready = self
            .ready
            .borrow()
            .as_ref()
            .filter(|r| Arc::ptr_eq(&r.mesh, &target.mesh))
            .cloned();
        if ready.is_some() {
            self.refresh();
        } else {
            self.prepare(target.mesh.clone());
        }
    }

    fn push_settings(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let s = self.settings.get();
        if let Some(t) = self.target.borrow().as_ref() {
            ui.set_cut_height_min(t.lo[2] as f32);
            ui.set_cut_height_max(t.hi[2] as f32);
        }
        ui.set_cut_height(s.height as f32);
        ui.set_cut_tilt_x(s.tilt[0] as f32);
        ui.set_cut_tilt_y(s.tilt[1] as f32);
        ui.set_cut_bounded(s.bounded);
        ui.set_cut_width(s.size[0] as f32);
        ui.set_cut_depth(s.size[1] as f32);
        ui.set_cut_offset_u(s.offset[0] as f32);
        ui.set_cut_offset_v(s.offset[1] as f32);
    }

    /// 0 height, 1 tilt about x, 2 tilt about y, 3 bounded (0/1), 4 width,
    /// 5 depth, 6 offset along the plane's first direction, 7 the second;
    /// -1 the settle timer (the plane stayed put: the cut is worked out).
    fn on_option(&self, code: i32, value: f32) {
        if code == -1 {
            self.start_trial();
            return;
        }
        let mut s = self.settings.get();
        let value = f64::from(value);
        match code {
            0 => s.height = value,
            1 => s.tilt[0] = value.clamp(-89.0, 89.0),
            2 => s.tilt[1] = value.clamp(-89.0, 89.0),
            3 => s.bounded = value != 0.0,
            4 => s.size[0] = value.max(0.1),
            5 => s.size[1] = value.max(0.1),
            6 => s.offset[0] = value,
            7 => s.offset[1] = value,
            _ => return,
        }
        if s == self.settings.get() {
            return;
        }
        self.settings.set(s);
        self.push_settings();
        self.refresh();
    }

    /// Makes `mesh` ready on a worker: checked closed, repaired when not.
    fn prepare(&self, mesh: Arc<IndexedMesh>) {
        if self
            .preparing
            .borrow()
            .as_ref()
            .is_some_and(|m| Arc::ptr_eq(m, &mesh))
        {
            return;
        }
        *self.preparing.borrow_mut() = Some(mesh.clone());
        self.shown.borrow_mut().take();
        self.ctrl.set_preview_overlay(None);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(true);
            ui.set_cut_can_cut(false);
            ui.set_cut_status(ctr!("CutRun", "Getting the model ready…"));
            ui.set_cut_warning("".into());
        }
        let id = self.job_id();
        self.waiting.set(id);
        let slot = self.slot.clone();
        let ui = self.ui.clone();
        std::thread::spawn(move || {
            let worked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match CutModel::new(mesh.clone()) {
                    Ok(model) => Ok(Ready {
                        mesh,
                        model,
                        repaired: false,
                    }),
                    Err(_) => {
                        let (_, fixed) = repaired_copy(&mesh, RepairPasses::default_repair());
                        let fixed =
                            fixed.ok_or_else(|| "the repair found nothing to do".to_owned())?;
                        let model = CutModel::new(Arc::new(fixed))
                            .map_err(|_| "it is not closed, even repaired".to_owned())?;
                        Ok(Ready {
                            mesh,
                            model,
                            repaired: true,
                        })
                    }
                }
            }))
            .unwrap_or_else(|payload| Err(panic_message(&payload)));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((id, Worked::Ready(worked)));
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_cut_worked());
        });
    }

    fn job_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// The section for the settings on show, drawn at once; the cut, once
    /// the plane settles.
    fn refresh(&self) {
        if !self.open.get() {
            return;
        }
        let Some(target) = self.target.borrow().clone() else {
            return;
        };
        let Some(ready) = self.ready.borrow().clone() else {
            return;
        };
        if !Arc::ptr_eq(&ready.mesh, &target.mesh) {
            return;
        }
        let settings = self.settings.get();
        let same_plane = self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.target.same(&target) && s.settings == settings);
        if !same_plane {
            self.toggled.borrow_mut().clear();
        }
        let Some(world) = placed(&settings, &target) else {
            return;
        };
        let Some(back) = target.world.inverse() else {
            return;
        };
        let local = Plane::through(
            back.apply(world.center),
            target.world.normal_back(world.plane.normal()),
        );
        let section = local.and_then(|plane| ready.model.section(plane).ok());
        let Some(section) = section else {
            self.shown.borrow_mut().take();
            self.show_overlay(&target, &world, false);
            self.say(
                ctr!("CutRun", "The plane does not cross the model."),
                Default::default(),
                false,
            );
            return;
        };
        let rect = Rect {
            center: [0.0; 2],
            half: world.half,
        };
        let to_rect = |p: [f64; 3]| {
            let w = target.world.apply(p);
            let d = [0, 1, 2].map(|k| w[k] - world.center[k]);
            [dot(d, world.u), dot(d, world.v)]
        };
        let from_rect = |q: [f64; 2]| {
            back.apply([0, 1, 2].map(|k| world.center[k] + world.u[k] * q[0] + world.v[k] * q[1]))
        };
        let fits = section.fits(&rect, to_rect, from_rect);
        let toggled = self.toggled.borrow();
        let chosen: Vec<bool> = fits
            .iter()
            .enumerate()
            .map(|(k, fit)| match fit {
                Fit::Crossed => false,
                Fit::Inside | Fit::Outside => {
                    toggled.get(&k).copied().unwrap_or(*fit == Fit::Inside)
                }
            })
            .collect();
        drop(toggled);
        let section = Arc::new(section);
        *self.shown.borrow_mut() = Some(Shown {
            target: target.clone(),
            settings,
            section: section.clone(),
            fits,
            chosen,
            trial: None,
            trial_job: 0,
        });
        self.show_overlay(&target, &world, true);
        self.describe();
        self.schedule_trial();
    }

    /// Works the cut out once the plane has stayed put a moment.
    fn schedule_trial(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let ui = ui.as_weak();
        self.settle
            .start(slint::TimerMode::SingleShot, SETTLE, move || {
                if let Some(ui) = ui.upgrade() {
                    ui.invoke_cut_option_changed(-1, 0.0);
                }
            });
    }

    fn start_trial(&self) {
        let id = self.job_id();
        let Some((section, chosen)) = self.shown.borrow_mut().as_mut().and_then(|s| {
            if s.trial.is_some() || !s.chosen.iter().any(|&c| c) {
                return None;
            }
            s.trial_job = id;
            Some((s.section.clone(), s.chosen.clone()))
        }) else {
            return;
        };
        let slot = self.slot.clone();
        let ui = self.ui.clone();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(true);
        }
        std::thread::spawn(move || {
            let cut =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| section.cut(&chosen)))
                    .unwrap_or_else(|payload| Err(CutError::Failed(panic_message(&payload))));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((id, Worked::Trial(cut)));
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_cut_worked());
        });
    }

    /// Takes a worker's result on the UI thread.
    fn worked(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some((id, worked)) = taken else {
            return;
        };
        // Only the job the tool still waits for counts: a model made ready
        // for the model on show, a cut for the section on show.
        let current = match worked {
            Worked::Ready(_) => id == self.waiting.get(),
            Worked::Trial(_) => self
                .shown
                .borrow()
                .as_ref()
                .is_some_and(|s| s.trial_job == id),
        };
        if !current {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_working(false);
        }
        match worked {
            Worked::Ready(ready) => {
                self.waiting.set(0);
                self.preparing.borrow_mut().take();
                match ready {
                    Ok(ready) => {
                        *self.ready.borrow_mut() = Some(Arc::new(ready));
                        self.refresh();
                    }
                    Err(why) => self.say(
                        ctr!("CutRun", "This model cannot be cut: {}", why),
                        Default::default(),
                        false,
                    ),
                }
            }
            Worked::Trial(cut) => match cut {
                Ok(cut) => {
                    if let Some(shown) = self.shown.borrow_mut().as_mut() {
                        shown.trial = Some(Arc::new(cut));
                    }
                    self.redraw();
                    self.describe();
                    if self.apply_when_in.replace(false) {
                        self.apply();
                    }
                }
                Err(e) => {
                    self.apply_when_in.set(false);
                    let why = match e {
                        CutError::NotClosedAfter => ctr!(
                            "CutRun",
                            "The pieces would not come out closed with only these parts of the cut. Take the whole plane, or move it a little."
                        ),
                        e => ctr!("CutRun", "The cut failed: {}", e.to_string()),
                    };
                    self.say(self.status_line(), why, false);
                }
            },
        }
    }

    /// The overlay for what is on show, redrawn.
    fn redraw(&self) {
        let (target, settings) = match self.shown.borrow().as_ref() {
            Some(s) => (s.target.clone(), s.settings),
            None => return,
        };
        if let Some(world) = placed(&settings, &target) {
            self.show_overlay(&target, &world, true);
        }
    }

    /// The plane, and the pieces of the section with what is to happen to
    /// each.
    fn show_overlay(&self, target: &Target, world: &Placed, with_section: bool) {
        let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
        let corner = |a: f64, b: f64| {
            [0, 1, 2].map(|k| (world.center[k] + world.u[k] * a + world.v[k] * b) as f32)
        };
        let [w, d] = world.half;
        pieces.push(Arc::new(OverlayMesh {
            positions: vec![corner(-w, -d), corner(w, -d), corner(w, d), corner(-w, d)],
            indices: vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2],
            tint: OverlayTint::Cavity,
        }));
        if with_section && let Some(shown) = self.shown.borrow().as_ref() {
            let joined: &[usize] = shown.trial.as_ref().map_or(&[], |t| &t.joined_by);
            // A hair off the plane, toward the camera's side either way, so
            // the plane does not hide them.
            let n = world.plane.normal();
            for (k, fit) in shown.fits.iter().enumerate() {
                let tint = if joined.contains(&k) {
                    OverlayTint::Invalid
                } else {
                    match fit {
                        Fit::Crossed => OverlayTint::Drain,
                        _ if shown.chosen[k] => OverlayTint::Valid,
                        _ => OverlayTint::Pending,
                    }
                };
                let (points, mut indices) = shown.section.fill(k);
                let base = indices.len();
                indices.extend_from_within(..base);
                for t in indices[base..].as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
                let positions = points
                    .iter()
                    .map(|p| {
                        let w = target.world.apply(p.map(f64::from));
                        [0, 1, 2].map(|i| (w[i] + n[i] * 0.01) as f32)
                    })
                    .collect();
                pieces.push(Arc::new(OverlayMesh {
                    positions,
                    indices,
                    tint,
                }));
            }
        }
        let overlay: Overlay = pieces.into();
        self.ctrl.set_preview_overlay(Some(overlay));
    }

    fn status_line(&self) -> slint::SharedString {
        let shown = self.shown.borrow();
        let Some(shown) = shown.as_ref() else {
            return Default::default();
        };
        let count = shown.fits.len();
        let taken = shown.chosen.iter().filter(|&&c| c).count();
        let crossed = shown.fits.iter().filter(|&&f| f == Fit::Crossed).count();
        let mut line = if taken == count {
            ctr!(
                "CutRun",
                "The plane crosses 1 part: it is cut.",
                "The plane crosses {n} parts: all are cut.",
                count
            )
            .to_string()
        } else {
            ctr!(
                "CutRun",
                "The plane crosses {} parts: {} cut, {} left whole.",
                count,
                taken,
                count - taken - crossed
            )
            .to_string()
        };
        if let Some(trial) = &shown.trial {
            line.push(' ');
            line.push_str(&match trial.pieces.len() {
                1 => ctr!("CutRun", "It leaves one piece.").to_string(),
                n => ctr!("CutRun", "It leaves {} pieces.", n).to_string(),
            });
        }
        line.into()
    }

    /// Says what the cut on show does, and whether it can be done.
    fn describe(&self) {
        let (warning, can) = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else {
                return;
            };
            let crossed = shown.fits.iter().filter(|&&f| f == Fit::Crossed).count();
            let taken = shown.chosen.iter().any(|&c| c);
            let one_piece = shown.trial.as_ref().is_some_and(|t| t.pieces.len() < 2);
            let ready_repaired = self.ready.borrow().as_ref().is_some_and(|r| r.repaired);
            let warning = if crossed > 0 {
                ctr!(
                    "CutRun",
                    "The plane's edge crosses a part (amber): cutting part of it would leave a slit. Move or resize the plane so it lies inside or outside."
                )
            } else if !taken {
                ctr!(
                    "CutRun",
                    "No part of the cut is taken. Click a part on the plane to take it."
                )
            } else if one_piece {
                ctr!(
                    "CutRun",
                    "This cut leaves one piece: the parts in red still join the two sides. Take them too, or make the plane reach them."
                )
            } else if ready_repaired {
                ctr!(
                    "CutRun",
                    "The model is not closed: it is repaired before it is cut."
                )
            } else {
                Default::default()
            };
            (warning, crossed == 0 && taken && !one_piece)
        };
        self.say(self.status_line(), warning, can);
    }

    fn say(&self, status: slint::SharedString, warning: slint::SharedString, can: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cut_status(status);
            ui.set_cut_warning(warning);
            ui.set_cut_can_cut(can);
        }
    }

    /// A click in the viewport while the tool is open: on a part of the
    /// cut, takes it or leaves it. `false` lets the click select as usual.
    fn click(&self, origin: [f64; 3], direction: [f64; 3]) -> bool {
        if !self.open.get() {
            return false;
        }
        let picked = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else {
                return false;
            };
            let Some(world) = placed(&shown.settings, &shown.target) else {
                return false;
            };
            let n = world.plane.normal();
            let along = dot(n, direction);
            if along.abs() < 1e-9 {
                return false;
            }
            let t = -world.plane.distance(origin) / along;
            if t < 0.0 {
                return false;
            }
            let hit = [0, 1, 2].map(|k| origin[k] + direction[k] * t);
            let Some(back) = shown.target.world.inverse() else {
                return false;
            };
            let Some(k) = shown.section.piece_at(back.apply(hit)) else {
                return false;
            };
            if shown.fits[k] == Fit::Crossed {
                return true;
            }
            (k, !shown.chosen[k])
        };
        let (k, take) = picked;
        self.toggled.borrow_mut().insert(k, take);
        if let Some(shown) = self.shown.borrow_mut().as_mut() {
            shown.chosen[k] = take;
            shown.trial = None;
            shown.trial_job = 0;
        }
        self.redraw();
        self.describe();
        self.schedule_trial();
        true
    }

    /// Cut: the model goes, its pieces come where it stood.
    pub(crate) fn apply(&self) {
        if !self.open.get() || !self.editable() {
            return;
        }
        let Some((target, trial)) = self
            .shown
            .borrow()
            .as_ref()
            .map(|s| (s.target.clone(), s.trial.clone()))
        else {
            return;
        };
        let Some(trial) = trial else {
            // Not worked out yet: it is, and is taken when it comes in.
            self.apply_when_in.set(true);
            self.settle.stop();
            self.start_trial();
            return;
        };
        if trial.pieces.len() < 2 {
            return;
        }
        let current = Target::selected(&self.document);
        if !current.is_some_and(|c| c.same(&target)) {
            self.refresh();
            return;
        }
        let leaf = target.leaf;
        let sessions = self.document.support_sessions();
        let Some((name, meta, supports)) = self
            .document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                Some((
                    leaf_name(graph, leaf),
                    graph.node(leaf)?.meta.clone(),
                    sets_of_model(spatial, &sessions, leaf),
                ))
            })
            .flatten()
        else {
            return;
        };
        let name = name.unwrap_or_else(|| ctr!("CutRun", "Object").to_string());
        let count = trial.pieces.len();
        let parts: Vec<(IndexedMesh, Option<SceneMetadata>)> = trial
            .pieces
            .iter()
            .enumerate()
            .map(|(k, mesh)| {
                let meta = SceneMetadata {
                    name: Some(ctr!("CutRun", "{} – part {}", name, k + 1).to_string()),
                    comment: meta.as_ref().and_then(|m| m.comment.clone()),
                };
                (mesh.clone(), Some(meta))
            })
            .collect();
        let slots = self.document.allocate_leaf_slots(count);
        let Some(edit) = self
            .document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                let split = split_edit(graph, leaf, parts, &slots)?;
                let mut edits = vec![split];
                if let Some(remove) = removal_edit(graph, &supports) {
                    edits.push(remove);
                }
                Some(SceneEdit::Compound(edits))
            })
            .flatten()
        else {
            return;
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                self.document.push_edit(edit);
                self.document
                    .set_selection(Selection::One(LeafId(slots[0].0)));
                crate::breadcrumbs::note(&format!(
                    "cut into {count} pieces{}",
                    if trial.by_fallback { " (fallback)" } else { "" }
                ));
                self.close();
                if let Some(ui) = self.ui.upgrade() {
                    let mut notice = ctr!("CutRun", "Cut into {} pieces.", count).to_string();
                    if !supports.is_empty() {
                        notice.push(' ');
                        notice.push_str(&ctr!(
                            "CutRun",
                            "Its supports were removed: generate supports for the pieces."
                        ));
                    }
                    ui.set_status_notice(notice.into());
                    let weak = ui.as_weak();
                    self.notice.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_secs(6),
                        move || {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_status_notice(Default::default());
                            }
                        },
                    );
                }
            }
            Err(EditNotApplied::NoSceneLoaded) => {}
            Err(EditNotApplied::Rejected(error)) => {
                let details = error.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("CutRun", "Cut"),
                    context: None,
                    message: ctr!("CutRun", "The cut could not be put in the scene."),
                    details: details.clone().into(),
                });
            }
        }
    }

    fn on_scene_changed(&self) {
        if !self.open.get() {
            return;
        }
        match Target::selected(&self.document) {
            None => self.close(),
            Some(target) => {
                let same = self
                    .target
                    .borrow()
                    .as_ref()
                    .is_some_and(|t| t.same(&target));
                if !same {
                    self.take_target(target, false);
                }
            }
        }
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
#[path = "tests_cut_run.rs"]
mod tests;
