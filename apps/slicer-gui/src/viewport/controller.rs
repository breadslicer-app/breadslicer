/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Coordinator for the per-viewport sub-controllers (`SessionState`,
//! `CameraController`, `RenderHost`, `InputDispatch`). Most public methods are
//! thin delegations to 1 of those.
//!
//! `handle_rendering_setup` is 1 of the exceptions: it touches both
//! `SessionState`, to publish the GPU device into the loader and drain any
//! queued pending load, and `RenderHost`, to install the renderer and flag a
//! redraw. The wgpu types appear here because the Slint rendering notifier
//! hands them over in its `RenderingSetup` state, and coordinating the session
//! and the render is the job of this module.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use sc_geom::df_hollow::hollow_df::LassoRegion;
use sc_mesh_core::transform::Transform3D;
use slint::ComponentHandle;
use slint::wgpu_30::wgpu;

use super::camera_controller::{CameraController, OrbitDrag};
use super::gesture_overlay::{GestureView, Screen};
use super::gizmo_drag::DragOp;
use super::gizmo_drag::GizmoState;
use super::gizmo_frame_composer::GizmoFrameComposer;
use super::group_move::GroupMove;
use super::input_dispatch::{InputDispatch, PointerUpOutcome, shows_supports};
use super::keyboard_drive::KbdFocus;
use super::modal_transform::{ModalGesture, ModalOp};
use super::numeric_transform::{
    GestureKind, NumericEdit, Readout, cheap_readout, gesture_delta_text, model_size, readout,
};
use super::object_list::{build_object_list_names, push_object_list};
use super::performance_probe::{Detail, ProbeResult, ProbeRun, ProbeUpdate};
use super::render_host::RenderHost;
use super::session_state::SessionState;
use super::support_drag::{DragPhase, DragPoint, SupportDragHandler, SupportDragKind};
use super::types::{FrameState, ScreenDelta, ScreenPoint};
use crate::MainWindow;
use crate::PointerModifiers;
use crate::WorkspaceSection;
use crate::build_volume::BuildVolume;
use crate::camera::AxisView;
use crate::clipboard;
use crate::clipboard::Clipboard;
use crate::cut_gizmo::{CutHandles, ScenePointer};
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::export_progress_panel::ExportProgressPanel;
use crate::gizmo;
use crate::gizmo::axis::Axis;
use crate::hollow_persist::install_hollows;
use crate::i18n::ctr;
use crate::mesh_check::show_card;
use crate::mesh_loader::{LoadDispatch, decide_load_dispatch, start_load_with_ticket};
use crate::mesh_render::{DeviceLoss, RenderError, RendererInitError, SceneRender, StageRefusal};
use crate::model_import::{
    self, ImportJob, OpeningJob, PreparedModels, WorkerDevice, add_to_scene, spawn_import,
    spawn_opening, spawn_placement,
};
use crate::pending_load::PendingLoad;
use crate::picking;
use crate::preferences_bus::PreferencesBus;
#[cfg(test)]
use crate::preferences_bus::PreferencesSnapshot;
use crate::print_file_preview;
use crate::save_progress_panel::SaveProgressPanel;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneEvent};
use crate::scene_loader::{Finished, LoadTicket, PreparedScene, SceneLoader};
use crate::selection::{LeafId, Selection};
use crate::support_persist::install_sets;
use crate::surface_brush::{BrushAim, SurfaceBrush, View, aim};
use crate::under_plate::{hidden_below_mm, sees_through_the_plate, under_plate_fade};
use sc_scene::SceneError;
use sc_sla_formats::{PrintPreviews, PrinterFileFormat};

/// How often the window is pinged while the overhang map waits for the
/// surface around a model's vertices, read on a worker (#498).
const MAP_WAIT_PING_MS: u64 = 100;

pub(crate) struct ViewportController {
    ui: slint::Weak<MainWindow>,
    session: SessionState,
    camera: CameraController,
    render: RenderHost,
    input: InputDispatch,
    /// Last cap key pushed to the slice-view panel image, so the paint path
    /// only rebuilds and re-uploads the cross-section when the selected layer
    /// or the slice generation actually changes. `None` means the panel shows
    /// an empty image. Shares the cap key space with the 3D cut so the 2 stay
    /// in lockstep.
    slice_view_key: Cell<Option<u64>>,
    /// The pixels of the review finding last clicked (#409), highlighted on
    /// the cut and in the slice-view panel, and a count of its changes for
    /// the cap key.
    review_highlight: RefCell<Option<Arc<sc_review::CavityFill>>>,
    highlight_generation: Cell<u64>,
    /// The name of the printer whose plate the viewport shows, drawn under
    /// the word on the plate's front tab; `None` for the placeholder plate.
    plate_printer_name: RefCell<Option<String>>,
    /// Set by every scene event that can move or reselect the object: the
    /// Transform inspector's numbers are read again on the next frame, at
    /// most once per frame however many drag ticks landed.
    readout_owed: Cell<bool>,
    /// The last full readout of the selected object and its world translation
    /// then: during a gesture the position follows the translation from it
    /// instead of walking the mesh every tick.
    readout_base: Cell<Option<(Readout, [f64; 3])>>,
    /// Preview pieces drawn over the scene (a support or drain before it is
    /// placed, a hollow cavity), set by the tab drivers.
    overlay: RefCell<Option<crate::mesh_render::Overlay>>,
    /// The marks of where supports are recommended (#524), drawn in the
    /// Supports tab.
    hint_marks: RefCell<Option<crate::mesh_render::Overlay>>,
    /// What a Repair run left broken, marked over the scene (#253).
    repair_marks: crate::repair_marks::MarksLayer,
    /// The overhang map's box, and the self-support angle it paints by.
    overhang_map_on: Cell<bool>,
    /// The Supports tab's box (#492): whether the rafts fade out with the
    /// plate when the camera goes under it (#481), and the hand tools reach
    /// past them.
    hide_rafts_under_plate: Cell<bool>,
    /// The Supports tab's box (#493): whether the supports are drawn as
    /// lines from under the plate, and those lines, gathered when the sets
    /// change.
    support_lines_on: Cell<bool>,
    support_lines: crate::support_lines::LinesCache,
    overhang_angle_deg: Cell<f32>,
    /// The support tips the map leaves what they hold plain by (#484), and
    /// the reach of the Supports tab's options, for a set that records none.
    overhang_tips: crate::overhang_tips::TipCache,
    tab_tip_reach_mm: Cell<f64>,
    /// The performance test while it runs. It owns the renderer's scene
    /// meanwhile, so the frame pass waits until it ends.
    probe: RefCell<Option<ProbeRun>>,
    /// Steps the performance test a slice at a time, so the window keeps
    /// answering while it runs.
    probe_timer: slint::Timer,
    /// Hides the pivot marker a moment after a numpad orbit step (#348).
    orbit_marker_timer: slint::Timer,
    /// While a model under the overhang map waits for the surface read on a
    /// worker (#498), a ping of the window now and then, for a frame to show
    /// it once it is ready.
    map_waits: Cell<bool>,
    map_wait_timer: slint::Timer,
    /// The Supports tab's drags (#3): who handles them, where a press of a
    /// drag tool stands, and a right press that dropped one (it opens no
    /// menu).
    support_drag_handler: RefCell<Option<std::rc::Weak<dyn SupportDragHandler>>>,
    support_drag: Cell<DragPhase>,
    right_dropped_drag: Cell<bool>,
    /// The model files that join the scene a load is opening, once it is
    /// open (#384).
    queued_imports: RefCell<Vec<PathBuf>>,
    /// Runs when Delete takes the scene's last object (#397).
    on_emptied: RefCell<Option<Rc<dyn Fn()>>>,
    /// The drag tool armed, for tests that run without a window.
    #[cfg(test)]
    drag_tool_for_test: Cell<Option<SupportDragKind>>,
    /// The brush that paints faces (#183), once a tool installs it.
    surface_brush: RefCell<Option<Rc<SurfaceBrush>>>,
    /// Whether the paint shows, and that what the renderer holds is old.
    paint_shown: Cell<bool>,
    paint_owed: Cell<bool>,
    /// A press of the brush that a stroke, a fill or nothing took: until
    /// the release, the press is the brush's.
    brush_press: Cell<bool>,
    /// A press off the model: the stroke starts where the drag reaches it,
    /// erasing or not.
    brush_waiting: Cell<Option<bool>>,
    /// The Hollow tab's lasso (#179): the points of a stroke in flight, in
    /// viewport pixels, and who takes the region it closes.
    lasso: RefCell<Option<Vec<[f32; 2]>>>,
    lasso_handler: RefCell<Option<LassoHandler>>,
    /// The Cut tool (#56): who is offered the pointer in the Transform tab
    /// first while it is open, and a press it took: where, and whether it
    /// has moved yet.
    cut_handles: RefCell<Option<Rc<dyn CutHandles>>>,
    cut_press: Cell<Option<([f32; 2], bool)>>,
    /// The focus of the last frame (#523), with the scene revision and the
    /// selection it was worked out for.
    focus_cache: FocusCache,
}

type FocusCache = RefCell<
    Option<(
        (crate::scene_document::SceneRevision, Vec<sc_scene::NodeId>),
        Vec<sc_scene::NodeId>,
    )>,
>;

/// Who takes the region a lasso stroke closes.
type LassoHandler = Rc<dyn Fn(LassoRegion)>;

/// How far a press the Cut tool took moves before it is a drag, pixels.
const CUT_DRAG_PX: f32 = 3.0;

/// A tool of a workspace tab that acts where the model is clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabTool {
    AddSupport,
    RemoveSupport,
    MoveTip,
    /// The brace tool (#3): a click on a support, then on another.
    Brace,
    Drain,
}

impl ViewportController {
    pub(crate) fn new(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        pending_load: Rc<PendingLoad>,
        device_loss: DeviceLoss,
        bus: &Rc<PreferencesBus>,
    ) -> Self {
        let dialogs = DialogManager::new(bus);
        dialogs.attach(ui);
        let loader = Rc::new(SceneLoader::new(ui, dialogs.clone(), device_loss));
        let save_progress = Rc::new(SaveProgressPanel::new(ui.as_weak()));
        let export_progress = Rc::new(ExportProgressPanel::new(ui.as_weak()));
        // A new window starts with the projection that the user last chose.
        let snapshot = bus.snapshot();
        let orthographic = snapshot.orthographic_view;
        ui.set_orthographic_view(orthographic);
        let input = InputDispatch::new();
        input.set_gizmo_look(gizmo::look::GizmoLook {
            style: snapshot.gizmo_style,
            size: gizmo::look::clamp_size(snapshot.gizmo_size) as f32 / 100.0,
            tool: gizmo::look::GizmoTool::Move,
        });
        ui.set_gizmo_focused_style(snapshot.gizmo_style == gizmo::look::GizmoStyle::Focused);
        Self {
            ui: ui.as_weak(),
            session: SessionState::new(
                document,
                loader,
                save_progress,
                export_progress,
                pending_load,
                dialogs,
            ),
            camera: CameraController::new(orthographic),
            render: RenderHost::new(ui),
            input,
            slice_view_key: Cell::new(None),
            review_highlight: RefCell::new(None),
            highlight_generation: Cell::new(0),
            plate_printer_name: RefCell::new(None),
            readout_owed: Cell::new(true),
            readout_base: Cell::new(None),
            overlay: RefCell::new(None),
            hint_marks: RefCell::new(None),
            repair_marks: Default::default(),
            overhang_map_on: Cell::new(false),
            hide_rafts_under_plate: Cell::new(true),
            support_lines_on: Cell::new(false),
            support_lines: Default::default(),
            overhang_angle_deg: Cell::new(default_overhang_angle_deg()),
            overhang_tips: Default::default(),
            tab_tip_reach_mm: Cell::new(default_tip_reach_mm()),
            probe: RefCell::new(None),
            probe_timer: slint::Timer::default(),
            orbit_marker_timer: slint::Timer::default(),
            map_waits: Cell::new(false),
            map_wait_timer: slint::Timer::default(),
            support_drag_handler: RefCell::new(None),
            support_drag: Cell::new(DragPhase::Idle),
            right_dropped_drag: Cell::new(false),
            queued_imports: RefCell::new(Vec::new()),
            on_emptied: RefCell::new(None),
            #[cfg(test)]
            drag_tool_for_test: Cell::new(None),
            surface_brush: RefCell::new(None),
            paint_shown: Cell::new(false),
            paint_owed: Cell::new(false),
            brush_press: Cell::new(false),
            brush_waiting: Cell::new(None),
            lasso: RefCell::new(None),
            lasso_handler: RefCell::new(None),
            cut_handles: RefCell::new(None),
            cut_press: Cell::new(None),
            focus_cache: RefCell::new(None),
        }
    }

    /// Builds a controller for unit tests. The stored `Weak` is dangling, so
    /// every `ui.upgrade()` returns `None` and the UI-mirroring side effects
    /// (`set_selected_object_index`, `set_object_list`, `request_redraw`)
    /// silently skip, leaving the in-Rust selection cell and dirty flag as the
    /// only observable state. This matches `SceneLoader::new_for_test`.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        let dialogs = DialogManager::new(&PreferencesBus::new(PreferencesSnapshot::for_tests()));
        let loader = Rc::new(SceneLoader::new_for_test(dialogs.clone()));
        let save_progress = Rc::new(SaveProgressPanel::new(slint::Weak::default()));
        let export_progress = Rc::new(ExportProgressPanel::new(slint::Weak::default()));
        let pending_load = Rc::new(PendingLoad::new());
        let controller = Self {
            ui: slint::Weak::default(),
            session: SessionState::new(
                SceneDocument::new(),
                loader,
                save_progress,
                export_progress,
                pending_load,
                dialogs,
            ),
            camera: CameraController::new(false),
            render: RenderHost::new_for_test(),
            input: InputDispatch::new(),
            slice_view_key: Cell::new(None),
            review_highlight: RefCell::new(None),
            highlight_generation: Cell::new(0),
            plate_printer_name: RefCell::new(None),
            readout_owed: Cell::new(true),
            readout_base: Cell::new(None),
            overlay: RefCell::new(None),
            hint_marks: RefCell::new(None),
            repair_marks: Default::default(),
            overhang_map_on: Cell::new(false),
            hide_rafts_under_plate: Cell::new(true),
            support_lines_on: Cell::new(false),
            support_lines: Default::default(),
            overhang_angle_deg: Cell::new(default_overhang_angle_deg()),
            overhang_tips: Default::default(),
            tab_tip_reach_mm: Cell::new(default_tip_reach_mm()),
            probe: RefCell::new(None),
            probe_timer: slint::Timer::default(),
            orbit_marker_timer: slint::Timer::default(),
            map_waits: Cell::new(false),
            map_wait_timer: slint::Timer::default(),
            support_drag_handler: RefCell::new(None),
            support_drag: Cell::new(DragPhase::Idle),
            right_dropped_drag: Cell::new(false),
            queued_imports: RefCell::new(Vec::new()),
            on_emptied: RefCell::new(None),
            #[cfg(test)]
            drag_tool_for_test: Cell::new(None),
            surface_brush: RefCell::new(None),
            paint_shown: Cell::new(false),
            paint_owed: Cell::new(false),
            brush_press: Cell::new(false),
            brush_waiting: Cell::new(None),
            lasso: RefCell::new(None),
            lasso_handler: RefCell::new(None),
            cut_handles: RefCell::new(None),
            cut_press: Cell::new(None),
            focus_cache: RefCell::new(None),
        };
        // Most viewport tests exercise the Transform-section tooling (gizmo
        // drag, keyboard drive, delete, paste); the fixture starts in
        // Transform, as production does. Section-gating tests set the section
        // they need explicitly.
        controller.session.set_section(WorkspaceSection::Transform);
        controller
    }

    /// A controller for tests that drive a window: as [`Self::new_for_test`],
    /// mirroring into `ui` and reading its tools.
    #[cfg(test)]
    pub(crate) fn new_for_test_with_ui(ui: &MainWindow) -> Self {
        let mut controller = Self::new_for_test();
        controller.ui = ui.as_weak();
        controller
    }

    /// Where `p` (world mm) lands on the viewport now, as a drag reads it.
    #[cfg(test)]
    pub(crate) fn project_for_test(&self, p: [f32; 3]) -> Option<[f32; 2]> {
        self.with_projection(|view| view.project(p)).flatten()
    }

    /// The model point a ray through `cursor` hits first, as the Add tool
    /// takes it.
    #[cfg(test)]
    pub(crate) fn model_hit_for_test(&self, cursor: ScreenPoint) -> Option<[f32; 3]> {
        self.tab_tool_hit(TabTool::AddSupport, cursor)
            .map(|(_, p, _)| [p.x, p.y, p.z])
    }

    /// Runs `with` with the view a drag sees now: what a box goes on
    /// looking through between the moves.
    pub(crate) fn with_drag_view<R>(
        &self,
        with: impl FnOnce(super::support_drag::Project<'_>) -> R,
    ) -> Option<R> {
        self.with_projection(with)
    }

    /// What a ray from the camera through pixel `at` meets first, as a box
    /// or a brush sees it.
    #[cfg(test)]
    pub(crate) fn first_hit_for_test(
        &self,
        at: [f32; 2],
    ) -> Option<super::support_drag::ScreenHit> {
        self.with_projection(|view| view.hits(&[at]).pop().flatten())
            .flatten()
    }

    /// The support point a ray through `cursor` hits first, as Remove and
    /// Move tip take it.
    #[cfg(test)]
    pub(crate) fn support_hit_for_test(&self, cursor: ScreenPoint) -> Option<[f32; 3]> {
        self.tab_tool_hit(TabTool::RemoveSupport, cursor)
            .map(|(_, p, _)| [p.x, p.y, p.z])
    }

    /// Installs a pre-built `Box<dyn MeshRenderer>` into the render-host's
    /// renderer slot, so a test can drive `handle_before_rendering` and the
    /// lifecycle hooks against a `MockRenderer`.
    #[cfg(test)]
    pub(crate) fn set_renderer_for_test(
        &self,
        renderer: Box<dyn crate::mesh_render::MeshRenderer>,
    ) {
        *self.render.renderer_cell().borrow_mut() = Some(renderer);
    }

    /// Sets the per-frame flag set to an arbitrary [`FrameState`], for the
    /// `handle_before_rendering` phase tests that need the upload-owed and
    /// redraw-owed combinations the production code reaches through document
    /// events.
    #[cfg(test)]
    pub(super) fn set_frame_state_for_test(&self, state: FrameState) {
        self.render.set_frame(state);
    }

    #[cfg(test)]
    pub(crate) fn install_graph_for_test(&self, graph: sc_scene::Scene) {
        // The tests ignore the source path. A sentinel in the slot keeps the
        // document invariant (scene present <=> path present) and makes any
        // assertion on `source_path` show its synthetic origin.
        self.session
            .document()
            .install_graph(graph, PathBuf::from("<test>"));
    }

    #[cfg(test)]
    pub(crate) fn overhang_map_for_test(&self) -> Option<crate::mesh_render::OverhangMap> {
        self.overhang_map()
    }

    #[cfg(test)]
    pub(crate) fn clear_frame_state_for_test(&self) {
        self.render.clear_frame_state();
    }

    #[cfg(test)]
    pub(crate) fn redraw_owed_for_test(&self) -> bool {
        self.render.redraw_owed()
    }

    /// Whether the renderer still owes the live scene an upload.
    #[cfg(test)]
    pub(crate) fn upload_owed_for_test(&self) -> bool {
        self.render.frame().upload_owed()
    }

    #[cfg(test)]
    pub(super) fn frame_state_for_test(&self) -> FrameState {
        self.render.frame()
    }

    #[cfg(test)]
    pub(crate) fn camera_is_initialised_for_test(&self) -> bool {
        self.camera.is_initialised()
    }

    #[cfg(test)]
    pub(super) fn renderer_installed_for_test(&self) -> bool {
        self.render.renderer_cell().borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_viewport_for_test(&self, w: f32, h: f32) {
        self.render.set_viewport(w, h);
    }

    /// Frames the camera on the build volume, as the first paint does before a
    /// scene is uploaded.
    #[cfg(test)]
    pub(crate) fn frame_camera_for_test(&self) {
        self.camera
            .replace_with_framing(None, self.session.build_volume());
    }

    #[cfg(test)]
    pub(crate) fn set_camera_for_test(&self, camera: crate::camera::OrbitCamera) {
        *self.camera.camera_cell().borrow_mut() = Some(camera);
    }

    #[cfg(test)]
    pub(crate) fn camera_for_test(&self) -> Option<crate::camera::OrbitCamera> {
        self.camera.camera_cell().borrow().clone()
    }

    #[cfg(test)]
    pub(crate) fn viewport_for_test(&self) -> Option<(f32, f32)> {
        self.render.viewport().map(|vp| (vp.width(), vp.height()))
    }

    #[cfg(test)]
    pub(super) fn keyboard_focus_for_test(&self) -> KbdFocus {
        self.input.kbd_drive().current_focus()
    }

    #[cfg(test)]
    /// Returns the printer name on the plate's front tab.
    pub(crate) fn plate_printer_name_for_test(&self) -> Option<String> {
        self.plate_printer_name.borrow().clone()
    }

    /// Returns the out-of-volume warning mask that a paint of the scene
    /// shows: 1 bit per envelope wall that some object crosses, 0 without a
    /// scene.
    #[cfg(test)]
    pub(crate) fn envelope_warning_mask_for_test(&self) -> u32 {
        self.session
            .document()
            .with_scene(|s| {
                crate::mesh_render::envelope_warning_mask_for_test(
                    s.graph(),
                    self.session.build_volume(),
                )
            })
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn reachable_leaf_count_for_test(&self) -> usize {
        self.session
            .document()
            .with_scene(|s| s.graph().reachable_leaf_count())
            .unwrap_or(0)
    }

    /// Returns the window's scene loader, whose file-load handshake (begin and
    /// end, stamp, slot) callers drive directly.
    pub(crate) fn loader(&self) -> Rc<SceneLoader> {
        self.session.loader().clone()
    }

    /// Returns the slot that holds a load request made before the window had a
    /// graphics device; `post_renderer_install` drains it.
    pub(crate) fn pending_load(&self) -> Rc<PendingLoad> {
        self.session.pending_load().clone()
    }

    pub(crate) fn document(&self) -> Rc<SceneDocument> {
        self.session.document().clone()
    }

    pub(crate) fn dialogs(&self) -> Rc<DialogManager> {
        self.session.dialogs().clone()
    }

    /// Returns the save-progress panel driver, which the async save flow
    /// shares.
    pub(crate) fn save_progress(&self) -> Rc<SaveProgressPanel> {
        self.session.save_progress().clone()
    }

    pub(crate) fn export_progress(&self) -> Rc<ExportProgressPanel> {
        self.session.export_progress().clone()
    }

    /// Subscribes the controller to its document. The listener captures a
    /// `Weak<Self>`, so the subscription makes no `Rc` cycle through the
    /// document.
    pub(crate) fn register_document_listener(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let sub = self.session.document().subscribe(move |event| {
            if let Some(strong) = weak.upgrade() {
                strong.handle_scene_event(event);
            }
        });
        self.session.install_subscription(sub);
    }

    /// Translates a `SceneEvent` into the viewport's reaction. It is the body
    /// of the document listener: other code drives the document and lets the
    /// listener react.
    fn handle_scene_event(&self, event: SceneEvent) {
        if !matches!(
            event,
            SceneEvent::SliceChanged | SceneEvent::DirtyChanged | SceneEvent::Painted
        ) {
            self.readout_owed.set(true);
        }
        if matches!(
            event,
            SceneEvent::Installed
                | SceneEvent::Cleared
                | SceneEvent::Restructured
                | SceneEvent::Painted
        ) {
            self.paint_owed.set(true);
        }
        match event {
            SceneEvent::Installed => self.on_scene_installed(),
            SceneEvent::Cleared => self.on_scene_cleared(),
            SceneEvent::Painted => self.mark_dirty(),
            SceneEvent::SelectionChanged => {
                self.on_modal_transform_finish(false);
                self.mirror_selection_to_ui();
                self.mark_dirty();
            }
            SceneEvent::Mutated => {
                // Every non-structural scene mutation routes through
                // `apply_edit`, so this is the single place that schedules a
                // redraw for them. The per-frame model-matrix upload reads the
                // live graph each frame, so a transform tick needs no extra
                // upload.
                self.mark_dirty();
            }
            SceneEvent::Restructured => {
                // A structural edit (leaf added or removed, mesh resource added
                // or removed, or a mesh replaced) just landed. The renderer's
                // GpuScene mirrors the arena leaf for leaf, so it needs a fresh
                // upload before the next draw; until then, `render` skips the
                // meshes when the leaf counts differ. The object-list side
                // panel also mirrors the arena and needs the same refresh.
                // `mark_dirty` flips redraw-owed for the redraw.
                self.render.update_frame(FrameState::with_upload_owed);
                if let Some(ui) = self.ui.upgrade() {
                    self.refresh_object_list_on(&ui);
                }
                self.mark_dirty();
            }
            SceneEvent::SliceChanged => {
                self.mirror_slice_layer_count_to_ui();
                self.mirror_slice_estimate_to_ui();
            }
            SceneEvent::UndoStackChanged => self.mirror_undo_state_to_ui(),
            SceneEvent::DirtyChanged => self.mirror_dirty_state_to_ui(),
        }
    }

    fn mirror_undo_state_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_can_undo(self.session.document().can_undo());
            ui.set_can_redo(self.session.document().can_redo());
        }
    }

    /// Pushes the unsaved-changes state to the UI, which prefixes the window
    /// title with `*` while it holds.
    fn mirror_dirty_state_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_scene_dirty(self.session.document().is_dirty());
        }
    }

    /// Pushes the held slice's layer count to the UI, sizing and enabling the
    /// height slider, and parks the selected layer at the top. Without a held
    /// slice the count is 0, which disables the slider. Runs on scene install
    /// and clear, where the document has just dropped any slice, and on every
    /// slice change; the slice driver also pushes the same values itself when a
    /// run completes.
    fn mirror_slice_layer_count_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let count = self.session.document().slice_layer_count().unwrap_or(0);
            ui.set_slice_layer_count(count as i32);
            ui.set_selected_layer_index(count.saturating_sub(1) as f32);
        }
    }

    /// Pushes the held slice's print time, resin and weight to the Slice tab,
    /// or why there is no estimate.
    fn mirror_slice_estimate_to_ui(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let document = self.session.document();
        if document.slice_layer_count().is_none() {
            ui.set_slice_estimate_valid(false);
            ui.set_slice_estimate_note(slint::SharedString::default());
            return;
        }
        match crate::export_print_file::estimate_held_slice(document) {
            Ok(e) => {
                ui.set_slice_estimate_valid(true);
                ui.set_slice_estimate_time(
                    crate::export_print_file::format_duration(e.time_sec).into(),
                );
                ui.set_slice_estimate_volume(format!("{:.1} ml", e.volume_ml).into());
                ui.set_slice_estimate_mass(format!("{:.1} g", e.mass_g).into());
                ui.set_slice_estimate_note(slint::SharedString::default());
            }
            Err(reason) => {
                ui.set_slice_estimate_valid(false);
                ui.set_slice_estimate_note(reason.into());
            }
        }
    }

    /// Pushes the open file's name to the UI, which shows it as the window
    /// title. Empty while no scene is loaded. Runs on scene install and clear;
    /// the save flow calls [`Self::refresh_document_title`] after a "Save As"
    /// moves the document to a new path.
    fn mirror_title_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let document = self.session.document();
            let name = document.source_file_name().unwrap_or_default();
            ui.set_document_name(name.into());
            // The project behind the title may have earlier versions (#18).
            ui.set_has_versions(crate::autosave::versions_dir_of(document).is_some());
        }
    }

    /// Re-reads the document's source path and updates the window title, for a
    /// "Save As" (or a first "Save" that fell back to it), which relocates the
    /// document without emitting a scene event.
    pub(crate) fn refresh_document_title(&self) {
        self.mirror_title_to_ui();
    }

    /// Builds the object names and the plate's front label again, in the
    /// language in force, for a language change.
    pub(crate) fn refresh_texts(&self) {
        if let Some(ui) = self.ui.upgrade() {
            self.refresh_object_list_on(&ui);
        }
        // The paint pass builds the label from the text in force.
        self.mark_dirty();
    }

    /// Builds the GPU scene again from the live graph, for a change in how
    /// meshes are drawn (smooth shading of models).
    pub(crate) fn upload_scene_again(&self) {
        self.render.update_frame(FrameState::with_upload_owed);
        self.mark_dirty();
    }

    /// Starts the performance test (Preferences, UI & Theme) in this viewport.
    /// `report` hears its progress and its result. Returns `false`, and
    /// reports nothing, when a test already runs.
    pub(crate) fn start_performance_probe(
        self: &Rc<Self>,
        report: Rc<dyn Fn(ProbeUpdate)>,
    ) -> bool {
        if self.probe.borrow().is_some() {
            return false;
        }
        let started = {
            let mut slot = self.render.renderer_cell().borrow_mut();
            match slot.as_mut() {
                Some(renderer) if !self.session.loader().device_loss().is_lost() => {
                    ProbeRun::start(renderer.as_mut(), self.session.build_volume(), Detail::FULL)
                        .ok()
                }
                _ => None,
            }
        };
        let Some(run) = started else {
            // A refused upload may have dropped the user's scene.
            self.upload_scene_again();
            report(ProbeUpdate::Failed);
            return true;
        };
        *self.probe.borrow_mut() = Some(run);
        report(ProbeUpdate::Progress(0.0));
        let controller = Rc::downgrade(self);
        self.probe_timer.start(
            slint::TimerMode::Repeated,
            core::time::Duration::from_millis(1),
            move || {
                if let Some(controller) = controller.upgrade() {
                    controller.step_performance_probe(&report);
                }
            },
        );
        true
    }

    /// Draws the next slice of the performance test, shows its last frame in
    /// the viewport and reports the progress, or ends the test.
    fn step_performance_probe(&self, report: &Rc<dyn Fn(ProbeUpdate)>) {
        /// How long one slice draws before the event loop runs again.
        const STEP_BUDGET: core::time::Duration = core::time::Duration::from_millis(40);

        let Some(ui) = self.ui.upgrade() else {
            return self.end_performance_probe(None, report);
        };
        let size = self.render.viewport().map(|vp| {
            let scale = ui.window().scale_factor();
            (
                (vp.width() * scale).round() as u32,
                (vp.height() * scale).round() as u32,
            )
        });
        let Some((width, height)) = size.filter(|(w, h)| *w > 0 && *h > 0) else {
            return self.end_performance_probe(None, report);
        };
        let stepped = {
            let mut slot = self.render.renderer_cell().borrow_mut();
            let mut probe = self.probe.borrow_mut();
            match (slot.as_mut(), probe.as_mut()) {
                (Some(renderer), Some(run)) => run
                    .step(renderer.as_mut(), width, height, STEP_BUDGET)
                    .map(|texture| (texture, run.is_done(), run.progress()))
                    .ok(),
                _ => None,
            }
        };
        let Some((texture, done, progress)) = stepped else {
            return self.end_performance_probe(None, report);
        };
        if let Some(image) = texture.and_then(|t| slint::Image::try_from(t).ok()) {
            ui.set_mesh_rendering(image);
        }
        if done {
            let run = self.probe.borrow_mut().take();
            let result = run.and_then(|run| run.finish(self.render.refresh_hz()));
            self.end_performance_probe(result, report);
        } else {
            report(ProbeUpdate::Progress(progress));
        }
    }

    /// Ends the performance test: drops its scene from the renderer, uploads
    /// the user's scene again and reports `result` (`None` for a test that
    /// could not finish).
    fn end_performance_probe(&self, result: Option<ProbeResult>, report: &Rc<dyn Fn(ProbeUpdate)>) {
        self.probe_timer.stop();
        self.probe.borrow_mut().take();
        if let Some(renderer) = self.render.renderer_cell().borrow_mut().as_mut() {
            renderer.clear_scene();
        }
        self.upload_scene_again();
        report(result.map_or(ProbeUpdate::Failed, ProbeUpdate::Finished));
    }

    /// Resets the viewport after a fresh scene install: the camera goes back to
    /// `None`, so the framing in `on_scene_load_finished` can set it, and the
    /// UI mirrors the new scene's object list and the cleared selection.
    fn on_scene_installed(&self) {
        self.input.modal.set(None);
        self.input.modal_revision.set(None);
        self.mirror_modal_to_ui();
        self.camera.reset();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_has_scene(true);
            self.refresh_object_list_on(&ui);
            ui.set_selected_object_index(-1);
            ui.set_gizmo_keyboard_focus_active(false);
        }
        self.mirror_undo_state_to_ui();
        self.mirror_dirty_state_to_ui();
        self.mirror_title_to_ui();
        self.mirror_slice_layer_count_to_ui();
        self.mark_dirty();
    }

    /// Rebuilds the object-list panel from the live graph and pushes it to
    /// `ui`. Runs on scene install and after every structural mutation, so the
    /// side panel stays in sync with the arena; non-structural edits (gizmo
    /// drag, keyboard nudge) do not change the list.
    fn refresh_object_list_on(&self, ui: &MainWindow) {
        let names = self
            .session
            .document()
            .with_scene(|spatial| build_object_list_names(spatial.graph()))
            .unwrap_or_default();
        push_object_list(ui, names);
    }

    /// Resets the viewport on scene close. The node that a drag captured is
    /// gone with the scene, so the FSM is reset without reverting the missing
    /// object's transform; the same holds for the keyboard integrator's focus
    /// and held-arrow bitset.
    fn on_scene_cleared(&self) {
        self.camera.reset();
        self.render.set_frame(FrameState::default());
        self.input.reset();
        self.mirror_modal_to_ui();
        self.render.clear_renderer_scene();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
            ui.set_has_scene(false);
            push_object_list(&ui, Vec::new());
            ui.set_selected_object_index(-1);
            ui.set_gizmo_keyboard_focus_active(false);
        }
        self.mirror_undo_state_to_ui();
        self.mirror_dirty_state_to_ui();
        self.mirror_title_to_ui();
        self.mirror_slice_layer_count_to_ui();
    }

    fn request_redraw(&self) {
        self.render.request_redraw();
    }

    /// Pings the window in a moment, to see whether the surface a model under
    /// the overhang map waits for is ready (#498).
    fn arm_map_wait(&self) {
        self.map_waits.set(true);
        let ui = self.ui.clone();
        self.map_wait_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(MAP_WAIT_PING_MS),
            move || {
                if let Some(ui) = ui.upgrade() {
                    ui.window().request_redraw();
                }
            },
        );
    }

    /// While the overhang map waits for a surface: a frame once it is ready,
    /// another ping until then, and no more once the last frame waited for
    /// none (the map off, the model gone).
    fn settle_map_wait(&self) {
        if !self.map_waits.get() {
            return;
        }
        let (pending, arrived) =
            self.render
                .renderer_cell()
                .borrow()
                .as_ref()
                .map_or((false, false), |renderer| {
                    (
                        renderer.overhang_map_pending(),
                        renderer.overhang_map_arrived(),
                    )
                });
        if !pending {
            self.map_waits.set(false);
        } else if arrived {
            self.map_waits.set(false);
            self.mark_dirty();
        } else {
            self.arm_map_wait();
        }
    }

    pub(crate) fn build_volume(&self) -> BuildVolume {
        self.session.build_volume()
    }

    /// Draws the previews a printer file of `format` carries, framed on the
    /// models.
    ///
    /// `None` when the format carries no previews, when no scene is loaded, or
    /// when the device cannot draw them.
    pub(crate) fn print_file_previews(&self, format: PrinterFileFormat) -> Option<PrintPreviews> {
        // TODO: this draws 2 frames and waits for the GPU to hand them back,
        // all on the UI thread, so the window stops repainting for as long as
        // that takes. A worker needs a second renderer on the same device,
        // which uploads the scene's geometry again unless 2 renderers can share
        // the installed buffers.
        let sizes = format.preview_sizes()?;
        let build_volume = self.build_volume();
        let mut cell = self.render.renderer_cell().borrow_mut();
        let renderer = cell.as_mut()?;
        self.session.document().with_scene(|spatial| {
            print_file_preview::render_previews(
                renderer.as_mut(),
                spatial.graph(),
                build_volume,
                sizes,
            )
        })?
    }

    /// Replaces the window's build volume and, when it changes, schedules a
    /// redraw. The renderer's plate-buffer cache keys on the `BuildVolume`, so
    /// the next render rebuilds the plate and the cage for the new envelope.
    /// The camera keeps its position.
    pub(crate) fn set_build_volume(&self, bv: BuildVolume) {
        if self.session.set_build_volume(bv) {
            // `mark_dirty` owes the redraw, which the render path needs before
            // it paints.
            self.mark_dirty();
        }
    }

    /// Replaces the name of the printer whose plate is shown and, when it
    /// changes, schedules a redraw: the paint pass draws it on the front tab.
    pub(crate) fn set_plate_printer_name(&self, name: Option<String>) {
        if *self.plate_printer_name.borrow() != name {
            *self.plate_printer_name.borrow_mut() = name;
            self.mark_dirty();
        }
    }

    /// Switches the active workspace section, mirrors it into the UI pill, and
    /// schedules a redraw. No-op when unchanged. Driven both by the pill
    /// switcher and by the load pipeline, which picks the initial section from
    /// mesh health. Entering Slice cancels any in-flight gizmo gesture so a
    /// half-finished transform cannot linger in a section that forbids editing.
    /// Leaving Supports takes the supports out of the selection: they are
    /// selected only there (#120). The models selected with them stay.
    pub(crate) fn set_section(&self, section: WorkspaceSection) {
        self.on_modal_transform_finish(false);
        if !self.session.set_section(section) {
            return;
        }
        crate::breadcrumbs::note(&format!("tab: {section:?}"));
        // The Review card is the Slice tab's: its highlight goes with it.
        if section != WorkspaceSection::Slice {
            self.set_review_highlight(None);
        }
        if !self.allows_transform_gizmo() {
            self.cancel_transform_gestures();
        }
        if !self.supports_selectable() {
            let document = self.session.document();
            let kept = document
                .with_scene(|spatial| {
                    document
                        .selection()
                        .retained(|id| !shows_supports(spatial, id))
                })
                .unwrap_or(Selection::None);
            if kept != document.selection() {
                self.input.gizmo_fsm().clear_hover();
                document.set_selection(kept);
            }
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_workspace_section(section);
        }
        self.mark_dirty();
    }

    /// Returns whether the active section permits the layout tools (gizmo,
    /// delete, copy, paste). The gating across the input paths reads this;
    /// selection and camera navigation ignore it.
    pub(crate) fn allows_layout_tools(&self) -> bool {
        crate::workspace_section::allows_layout_tools(self.session.section())
    }

    fn allows_transform_gizmo(&self) -> bool {
        crate::workspace_section::allows_transform_gizmo(self.session.section())
    }

    /// Whether a gizmo drag, a keyboard transform or a live preview (#182)
    /// is under way: the scene then shows a pose no edit has committed yet.
    pub(crate) fn is_mid_gesture(&self) -> bool {
        self.session.document().has_live_preview()
            || self.brush_press.get()
            || self.input.modal.get().is_some()
            || matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. })
    }

    /// Returns whether the active section permits undo and redo. Wider than the
    /// layout gate: history stays reachable in Prepare (mesh repair is undoable
    /// there) and is blocked only in Slice.
    pub(crate) fn allows_undo_redo(&self) -> bool {
        crate::workspace_section::allows_undo_redo(self.session.section())
    }

    /// Reverts an in-progress gizmo drag and drops keyboard gizmo focus,
    /// leaving the selection intact. Called when the section changes to one
    /// that forbids transforms.
    fn cancel_transform_gestures(&self) {
        self.on_modal_transform_finish(false);
        if matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. }) {
            self.input.abort_gizmo_drag(self.session.document());
        }
        if self.input.kbd_drive().current_focus() != KbdFocus::None {
            self.apply_keyboard_focus(KbdFocus::None);
        }
    }

    fn mark_dirty(&self) {
        self.render.mark_dirty();
    }

    /// Whether the overlay now shown is `pieces`.
    pub(crate) fn preview_overlay_is(&self, pieces: &crate::mesh_render::Overlay) -> bool {
        self.overlay
            .borrow()
            .as_ref()
            .is_some_and(|shown| Arc::ptr_eq(shown, pieces))
    }

    /// The marks of where supports are recommended now (#524).
    #[cfg(test)]
    pub(crate) fn hint_marks_for_test(&self) -> Option<crate::mesh_render::Overlay> {
        self.hint_marks.borrow().clone()
    }

    /// Shows the marks of where supports are recommended (#524) until
    /// replaced (`None` clears them).
    pub(crate) fn set_hint_marks(&self, marks: Option<crate::mesh_render::Overlay>) {
        *self.hint_marks.borrow_mut() = marks;
        self.mark_dirty();
    }

    /// Shows `pieces` over the scene until replaced (`None` clears them).
    pub(crate) fn set_preview_overlay(&self, pieces: Option<crate::mesh_render::Overlay>) {
        let changed = match (&*self.overlay.borrow(), &pieces) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            *self.overlay.borrow_mut() = pieces;
            self.mark_dirty();
        }
    }

    /// Marks what a Repair run left broken over the scene, in the Transform
    /// tab, until replaced (`None` clears them).
    pub(crate) fn set_repair_marks(&self, marks: Option<crate::mesh_render::Overlay>) {
        if self.repair_marks.set(marks) {
            self.mark_dirty();
        }
    }

    /// Turns the overhang map on or off (#91).
    pub(crate) fn set_overhang_map(&self, on: bool) {
        if self.overhang_map_on.replace(on) != on {
            self.mark_dirty();
        }
    }

    /// Whether the rafts fade out with the plate when the camera goes under
    /// it (#492). Off, they stay drawn from under the plate and the hand
    /// tools stop on them; the plate still fades.
    pub(crate) fn set_hide_rafts_under_plate(&self, on: bool) {
        if self.hide_rafts_under_plate.replace(on) != on {
            self.mark_dirty();
        }
    }

    /// Whether the supports are drawn as lines from under the plate (#493).
    pub(crate) fn set_support_lines_under_plate(&self, on: bool) {
        if self.support_lines_on.replace(on) != on {
            self.mark_dirty();
        }
    }

    /// Whether `camera` sees past the rafts: under the plate, with the
    /// Supports tab's box on (#481, #492).
    fn sees_past_rafts(&self, camera: &crate::camera::OrbitCamera) -> bool {
        self.hide_rafts_under_plate.get() && sees_through_the_plate(camera)
    }

    /// The self-support angle the overhang map paints by, degrees from
    /// horizontal: the one the support engine is set to.
    pub(crate) fn set_overhang_angle(&self, degrees: f32) {
        if self.overhang_angle_deg.replace(degrees) != degrees {
            self.mark_dirty();
        }
    }

    /// How far around itself a tip of a set that records no options holds
    /// an overhang, mm: the Supports tab's options' (#484).
    pub(crate) fn set_tip_reach(&self, mm: f64) {
        if self.tab_tip_reach_mm.replace(mm) != mm {
            self.mark_dirty();
        }
    }

    #[cfg(test)]
    pub(crate) fn support_tips_for_test(&self) -> Rc<[crate::mesh_render::SupportTip]> {
        self.support_tips()
    }

    /// The support tips of the scene, for the overhang map (#484).
    fn support_tips(&self) -> Rc<[crate::mesh_render::SupportTip]> {
        self.overhang_tips
            .tips(self.session.document(), self.tab_tip_reach_mm.get())
    }

    #[cfg(test)]
    pub(crate) fn raft_top_under_plate_for_test(&self) -> f32 {
        self.raft_top_under_plate()
    }

    /// What the overhang map shows in the current section, if anything.
    /// How high the rafts reach while the camera is under the plate, for the
    /// view to fade them with it (#481); 0 with the camera above it.
    fn raft_top_under_plate(&self) -> f32 {
        if !self.hide_rafts_under_plate.get() || !self.under_plate() {
            return 0.0;
        }
        self.raft_top()
    }

    /// Whether the camera is under the plate, which fades (#481).
    fn under_plate(&self) -> bool {
        self.camera
            .camera_cell()
            .borrow()
            .as_ref()
            .is_some_and(|camera| under_plate_fade(camera) > 0.0)
    }

    /// How high the rafts of the scene reach, mm (0 without one).
    fn raft_top(&self) -> f32 {
        let document = self.session.document();
        let sessions = document.support_sessions();
        document
            .with_scene(|spatial| crate::support_persist::raft_top_mm(spatial, &sessions))
            .unwrap_or(0.0)
    }

    /// The supports as lines from under the plate (#493): the height above
    /// which the support meshes fade (`f32::MAX` keeps them; below 0 fades
    /// them whole, rafts and all, when the rafts fade too or there is none)
    /// and the lines drawn in their place. Kept as they are with the box off
    /// or the camera above the plate.
    fn under_plate_supports(&self) -> (f32, Option<Arc<crate::mesh_render::SupportLines>>) {
        if !self.support_lines_on.get() || !self.under_plate() {
            return (f32::MAX, None);
        }
        let raft_top = self.raft_top();
        let shown_up_to = if self.hide_rafts_under_plate.get() || raft_top <= 0.0 {
            -1.0
        } else {
            hidden_below_mm(raft_top)
        };
        let lines = self.support_lines.lines(self.session.document());
        (shown_up_to, Some(lines))
    }

    /// Whether the supports are left out of the view: while the Hollow tab's
    /// lasso is armed, the model stands clear to draw around (#512).
    fn supports_hidden(&self) -> bool {
        self.session.section() == WorkspaceSection::Hollow
            && self
                .ui
                .upgrade()
                .is_some_and(|ui| ui.get_hollow_lasso_tool())
    }

    #[cfg(test)]
    pub(crate) fn under_plate_supports_for_test(
        &self,
    ) -> (f32, Option<Arc<crate::mesh_render::SupportLines>>) {
        self.under_plate_supports()
    }

    fn overhang_map(&self) -> Option<crate::mesh_render::OverhangMap> {
        (self.overhang_map_on.get()
            && crate::workspace_section::shows_overhang_map(self.session.section()))
        .then(|| crate::mesh_render::OverhangMap::new(self.overhang_angle_deg.get()))
    }

    /// Repaints after the slice height slider moved, so the mesh clip follows
    /// the new layer; the paint reads the clip height from the current UI
    /// value.
    pub(crate) fn on_selected_layer_changed(&self) {
        self.mark_dirty();
    }

    /// Highlights the pixels of a review finding (#409) on the cut and in
    /// the slice-view panel, on whichever of its layers the slider shows,
    /// until replaced (`None` clears it). Only the selected layer's cap is
    /// made again; the scene is not uploaded again.
    pub(crate) fn set_review_highlight(&self, footprint: Option<Arc<sc_review::CavityFill>>) {
        let changed = match (&*self.review_highlight.borrow(), &footprint) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            *self.review_highlight.borrow_mut() = footprint;
            self.highlight_generation
                .set(self.highlight_generation.get().wrapping_add(1));
            self.mark_dirty();
        }
    }

    /// The pixels highlighted now (#409).
    #[cfg(test)]
    pub(crate) fn review_highlight(&self) -> Option<Arc<sc_review::CavityFill>> {
        self.review_highlight.borrow().clone()
    }

    /// The cap of layer `layer` of the held slice, with the highlighted
    /// finding's pixels on that layer marked (#409).
    pub(crate) fn cap_raster(
        &self,
        layer: u32,
        build_volume: &BuildVolume,
    ) -> Option<crate::slice_cap::CapRaster> {
        let document = self.session.document();
        let (mut raster, z_mm) = document
            .with_slice_layer(layer, |l| {
                crate::slice_cap::build_cap_raster(l, build_volume).map(|r| (r, l.z_mm()))
            })
            .flatten()?;
        if let Some(footprint) = self.review_highlight.borrow().as_ref() {
            let rects: Vec<[f64; 4]> = footprint
                .slabs
                .iter()
                .filter(|slab| slab.bottom_mm <= z_mm && z_mm < slab.top_mm)
                .flat_map(|slab| {
                    slab.rects
                        .iter()
                        .map(|r| [r.x0_mm, r.y0_mm, r.x1_mm, r.y1_mm])
                })
                .collect();
            if !rects.is_empty() {
                crate::slice_cap::highlight_cap(&mut raster, &rects, build_volume);
            }
        }
        Some(raster)
    }

    pub(crate) fn on_viewport_resized(&self, w: f32, h: f32) {
        self.render.on_viewport_resized(w, h);
    }

    /// Drops the bucketed MSAA and target allocations once `viewport-resized`
    /// has stayed quiet for `RESIZE_SETTLE`, so the next frame reallocates at
    /// the exact logical size and the `image-fit: fill` stretch of the drag
    /// ends.
    pub(crate) fn on_resize_settled(&self) {
        self.render.on_resize_settled();
    }

    /// A right or middle button went down to orbit, with the cursor at
    /// `(x, y)`. The pivot is chosen at the first movement (#346).
    pub(crate) fn on_orbit_begin(&self, x: f32, y: f32) {
        self.orbit_marker_timer.stop();
        self.camera.arm_orbit(ScreenPoint::new(x, y));
    }

    /// A numpad key turns the camera one step (#348) about the pivot of the
    /// drag in progress, else the selection, else the camera's target. The
    /// marker shows the pivot for a moment.
    pub(crate) fn on_orbit_step(&self, horizontal: i32, vertical: i32) {
        let Some(vp) = self.render.viewport() else {
            return;
        };
        let held = match self.camera.orbit_drag() {
            OrbitDrag::Pivot(pivot) => Some(pivot),
            OrbitDrag::Idle | OrbitDrag::Armed(_) => None,
        };
        let Some(pivot) = held.or_else(|| {
            let slot = self.camera.camera_cell().borrow();
            let camera = slot.as_ref()?;
            Some(super::orbit_pivot::step_pivot(
                self.session.document(),
                camera,
            ))
        }) else {
            return;
        };
        let (yaw, pitch) = super::orbit_pivot::step_angles(horizontal, vertical);
        if !self.camera.turn_around(pivot, yaw, pitch) {
            return;
        }
        self.mark_dirty();
        self.show_orbit_marker(pivot, vp);
        if held.is_none() {
            let ui = self.ui.clone();
            self.orbit_marker_timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(700),
                move || {
                    if let Some(ui) = ui.upgrade() {
                        ui.set_orbit_pivot_visible(false);
                    }
                },
            );
        }
    }

    /// The orbit's button came up: the pivot and its marker go.
    pub(crate) fn on_orbit_end(&self) {
        if self.camera.end_orbit()
            && let Some(ui) = self.ui.upgrade()
        {
            ui.set_orbit_pivot_visible(false);
        }
    }

    pub(crate) fn on_rotate(&self, delta: ScreenDelta) {
        if let OrbitDrag::Armed(cursor) = self.camera.orbit_drag() {
            self.hold_orbit_pivot(cursor);
        }
        if self.camera.on_rotate(delta, self.render.viewport()) {
            self.mark_dirty();
        }
    }

    /// Chooses the pivot of the armed orbit and shows its marker.
    fn hold_orbit_pivot(&self, cursor: ScreenPoint) {
        let Some(vp) = self.render.viewport() else {
            return;
        };
        let slot = self.camera.camera_cell().borrow();
        let Some(camera) = slot.as_ref() else {
            return;
        };
        let (pivot, _) = super::orbit_pivot::choose(
            self.session.document(),
            camera,
            vp,
            self.session.build_volume(),
            cursor,
        );
        drop(slot);
        self.camera.hold_orbit_pivot(pivot);
        self.show_orbit_marker(pivot, vp);
    }

    /// Shows the pivot marker: on the pivot, or on the viewport's edge
    /// pointing toward it when it is off screen (#349). The pivot holds its
    /// place on screen while the camera turns about it, so once is enough.
    fn show_orbit_marker(&self, pivot: glam::Vec3, vp: super::types::ViewportSize) {
        let Some(marker) = self
            .camera
            .camera_cell()
            .borrow()
            .as_ref()
            .map(|camera| super::orbit_pivot::marker(camera, vp, pivot))
        else {
            return;
        };
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orbit_pivot_x(marker.x);
            ui.set_orbit_pivot_y(marker.y);
            ui.set_orbit_pivot_edge(marker.toward.is_some());
            ui.set_orbit_pivot_toward(marker.toward.unwrap_or(0.0));
            ui.set_orbit_pivot_visible(true);
        }
    }

    pub(crate) fn on_pan(&self, delta: ScreenDelta) {
        if self.camera.on_pan(delta, self.render.viewport()) {
            self.mark_dirty();
        }
    }

    pub(crate) fn on_zoom(&self, wheel_delta: f32, cursor: ScreenPoint) {
        if self
            .camera
            .on_zoom(wheel_delta, cursor, self.render.viewport())
        {
            self.mark_dirty();
        }
    }

    /// Switches between the perspective and the orthographic view. The
    /// projection outlives the scene: loading or closing one keeps it. Returns
    /// whether the view is now orthographic, for the caller to store.
    pub(crate) fn on_toggle_projection(&self) -> bool {
        let orthographic = self.camera.toggle_projection();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orthographic_view(orthographic);
        }
        self.mark_dirty();
        orthographic
    }

    /// Returns whether the view is orthographic.
    #[cfg(test)]
    pub(crate) fn orthographic_view(&self) -> bool {
        self.camera.orthographic()
    }

    pub(crate) fn on_frame_selected(&self) {
        let Some(vp) = self.render.viewport() else {
            return;
        };
        let document = self.session.document();
        let Some(id) = document.selection_primary_node_id() else {
            return;
        };
        let bounds = document
            .with_scene(|spatial| {
                let query = spatial.leaf(id)?;
                query.transformed_aabb(query.world_transform())
            })
            .flatten();
        if let Some(bounds) = bounds
            && let Some(cam) = self.camera.camera_cell().borrow_mut().as_mut()
        {
            cam.frame_bounds(
                glam::Vec3::from(bounds.min().coords()),
                glam::Vec3::from(bounds.max().coords()),
                crate::mesh_render::default_fov_y(),
                vp.width() / vp.height().max(1.0),
            );
            self.mark_dirty();
        }
    }

    /// Centres the camera on `point` (world mm), keeping the view direction;
    /// a far camera comes in to frame about `radius` around it. For the
    /// Review card's jump to a finding.
    pub(crate) fn centre_camera_on(&self, point: glam::Vec3, radius: f32) {
        let Some(vp) = self.render.viewport() else {
            return;
        };
        if let Some(cam) = self.camera.camera_cell().borrow_mut().as_mut() {
            cam.centre_on(
                point,
                radius,
                crate::mesh_render::default_fov_y(),
                vp.width() / vp.height().max(1.0),
            );
            self.mark_dirty();
        }
    }

    pub(crate) fn on_frame_view(&self) {
        if self.camera.on_frame_view() {
            self.mark_dirty();
        }
    }

    pub(crate) fn on_axis_view(&self, view: crate::AxisView) {
        if self.camera.on_axis_view(axis_from_ui(view)) {
            self.mark_dirty();
        }
    }

    pub(crate) fn on_close_scene(&self) {
        // The loader's invalidation belongs to the close gesture, so it runs
        // here. The `clear_scene` call below fires `SceneEvent::Cleared`, and
        // the controller's listener handles the full viewport-state reset
        // (camera, frame, gizmo, keyboard focus, renderer, UI).
        self.session.loader().invalidate_and_clear();
        self.queued_imports.borrow_mut().clear();
        self.session.document().clear_scene();
    }

    /// Queues `path` for the window or starts its load, in place of the
    /// scene. A window busy loading drops it.
    pub(crate) fn dispatch_load(&self, path: PathBuf) {
        let loader = self.session.loader();
        if decide_load_dispatch(loader.graphics_unavailable()) == LoadDispatch::Queue {
            // Pre-first-paint: `post_renderer_install` dispatches this once
            // `handle_rendering_setup` publishes the device.
            self.session.pending_load().queue(path);
            return;
        }
        let Some(ticket) = loader.begin_load() else {
            // A drop onto a busy window is silently ignored.
            return;
        };
        // Covers both a ready device and a renderer that will never provide
        // one; `start_load_with_ticket` reads the same graphics state and
        // deposits the reason for the latter, so a path that can never open
        // reports its failure at once.
        start_load_with_ticket(ticket, path, loader, self.session.build_volume());
    }

    /// Opens the model files `paths` (#384). With a scene open they join it,
    /// on free room of the plate, as one undo step. Without one, they open
    /// one together (#391): the first file read is the scene, as it opens
    /// alone, the others join it, as part of the opening, and the window
    /// shows them all at once.
    pub(crate) fn open_models(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.session.document().is_loaded() {
            self.start_import(paths, true);
            return;
        }
        let loader = self.session.loader();
        if decide_load_dispatch(loader.graphics_unavailable()) == LoadDispatch::Queue {
            // Pre-first-paint: the first waits for the device as any file
            // does, and the others with it, in the window's queue.
            let mut paths = paths.into_iter();
            if let Some(first) = paths.next() {
                *self.queued_imports.borrow_mut() = paths.collect();
                self.session.pending_load().queue(first);
            }
            return;
        }
        let Some(ticket) = loader.begin_load() else {
            // A drop onto a busy window is silently ignored.
            return;
        };
        self.start_opening(ticket, paths);
    }

    /// Opens `paths`, never empty, in place of the scene: one file as any
    /// file opens, several together as one scene, read on one worker.
    fn start_opening(&self, ticket: LoadTicket, paths: Vec<PathBuf>) {
        let loader = self.session.loader();
        let volume = self.session.build_volume();
        let device = loader.gpu_snapshot();
        let (Some((device, token, _queue)), [_, _, ..]) = (device, paths.as_slice()) else {
            // One file; or no device, which the load of the first reports,
            // as it would for that file alone.
            if let Some(first) = paths.into_iter().next() {
                start_load_with_ticket(ticket, first, loader, volume);
            }
            return;
        };
        crate::breadcrumbs::started("load");
        crate::breadcrumbs::note(&format!("load: {} files together", paths.len()));
        let job = OpeningJob {
            paths,
            plate: crate::arrange_run::plate_of(&volume),
            build_volume: volume.volume(),
            revision: self.session.document().scene_revision(),
            device: WorkerDevice {
                device,
                token,
                loss: loader.device_loss(),
            },
        };
        loader.begin_progress_ui();
        spawn_opening(ticket, job, loader.progress_sink());
    }

    /// Starts the load `pending` once the window has a device, with the
    /// files queued to open with it.
    fn start_pending_load(&self, pending: PathBuf) {
        let queued = std::mem::take(&mut *self.queued_imports.borrow_mut());
        if let Some(ticket) = self.session.loader().begin_load() {
            self.start_opening(ticket, std::iter::once(pending).chain(queued).collect());
        }
    }

    /// Reads `paths` on a worker, to join the open scene; recorded as an
    /// undo step when `recorded`. A window busy loading drops them.
    fn start_import(&self, paths: Vec<PathBuf>, recorded: bool) {
        let loader = self.session.loader();
        let Some(ticket) = loader.begin_load() else {
            return;
        };
        let document = self.session.document();
        let volume = self.session.build_volume();
        crate::breadcrumbs::started("import");
        crate::breadcrumbs::note(&format!("import: {} file(s)", paths.len()));
        let device = loader
            .gpu_snapshot()
            .map(|(device, token, _queue)| WorkerDevice {
                device,
                token,
                loss: loader.device_loss(),
            });
        let job = ImportJob {
            paths,
            standing: crate::arrange_run::standing_bodies(document),
            plate: crate::arrange_run::plate_of(&volume),
            build_volume: volume.volume(),
            revision: document.scene_revision(),
            recorded,
            device,
        };
        loader.begin_progress_ui();
        spawn_import(ticket, job, loader.progress_sink());
    }

    /// Adds the models an import read to the scene, selects them and says
    /// what could not be read or does not fit.
    fn finish_import(&self, mut models: PreparedModels) {
        let document = self.session.document();
        // The plate changed while the files were read: they are placed
        // again, off this thread, against it as it stands now.
        if document.scene_revision() != models.revision
            && !models.files.is_empty()
            && let Some(ticket) = self.session.loader().begin_load()
        {
            crate::breadcrumbs::note("import: the plate changed, placed again");
            spawn_placement(
                ticket,
                models,
                crate::arrange_run::standing_bodies(document),
                crate::arrange_run::plate_of(&self.session.build_volume()),
                document.scene_revision(),
            );
            return;
        }
        crate::breadcrumbs::finished(
            "import",
            &format!(
                "{} read, {} failed",
                models.files.len(),
                models.failures.len()
            ),
        );
        self.on_modal_transform_finish(false);
        // The meshes the worker uploaded, for the upload the added models owe
        // to take instead of uploading them on this thread.
        if let Some(renderer) = self.render.renderer_cell().borrow_mut().as_mut() {
            for meshes in models.meshes.drain(..) {
                renderer.adopt_meshes(meshes);
            }
        }
        let placements = models.placements.clone();
        match add_to_scene(
            document,
            &models.files,
            &placements,
            &models.primed,
            models.recorded,
        ) {
            Ok(added) if !added.is_empty() => {
                // Each file keeps its import notes, with the scene's.
                self.session.loader().note_imported(&models);
                self.set_section(WorkspaceSection::Transform);
                let check = document.mesh_check();
                for file in &models.files {
                    for (mesh, sliceable) in &file.verdicts {
                        check.remember(mesh, *sliceable, None);
                    }
                }
                if let (Some(ui), Some(Some(card))) = (
                    self.ui.upgrade(),
                    document.with_scene(|spatial| check.card(spatial.graph(), false)),
                ) {
                    show_card(&ui, &card);
                }
            }
            Ok(_) => {}
            Err(error) => self.show_edit_rejected(error),
        }
        self.report_import_failures(&models.failures);
        self.report_left_off(model_import::left_off(&models, &placements));
    }

    /// Says which files of an import do not fit on the plate.
    fn report_left_off(&self, left_off: Vec<String>) {
        if !left_off.is_empty() {
            let count = left_off.len();
            self.session.dialogs().show_info(move || InfoTexts {
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

    /// Says which files of an import could not be read, and why.
    fn report_import_failures(&self, failures: &[(PathBuf, String)]) {
        match failures {
            [] => {}
            [(path, why)] => {
                let (path, why) = (path.clone(), why.clone());
                self.session.dialogs().show_error(move || ErrorTexts {
                    heading: ctr!("ErrorDialog", "Failed to open 3D model"),
                    context: Some(ctr!("ErrorDialog", "File: {}", path.display())),
                    message: why.as_str().into(),
                    details: Default::default(),
                });
            }
            many => {
                let count = many.len();
                let details = many
                    .iter()
                    .map(|(path, why)| format!("{}: {why}", model_import::file_name(path)))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.session.dialogs().show_error(move || ErrorTexts {
                    heading: ctr!("ErrorDialog", "Failed to open 3D model"),
                    context: None,
                    message: ctr!(
                        "ErrorDialog",
                        "{n} file could not be opened.",
                        "{n} files could not be opened.",
                        count
                    ),
                    details: details.as_str().into(),
                });
            }
        }
    }

    pub(crate) fn on_scene_load_finished(&self) {
        let prepared = match self.session.loader().take_finished_load() {
            Some(Finished::Scene(prepared)) => prepared,
            Some(Finished::Models(models)) => {
                self.finish_import(models);
                return;
            }
            // The file could not be opened, and the error dialog is up;
            // nothing, or a load dropped by a close.
            Some(Finished::Failed) | None => return,
        };
        crate::breadcrumbs::finished("load", "ok");
        let PreparedScene {
            spatial,
            gpu_scene,
            project_payload,
            reloaded_slices,
            source_path,
            mesh_verdicts,
            restored_supports,
            restored_hollows,
            joined,
            ..
        } = prepared;

        // Try to install the worker-built buffers directly. Falls through to
        // the UI-thread upload path if the renderer is gone (post-teardown) or
        // now uses a different device than the one the worker built for
        // (mid-load device loss); the `device_token` carried by `gpu_scene` is
        // what tells `install_scene` whether the worker's snapshot still
        // matches.
        let scene_installed = self
            .render
            .renderer_cell()
            .borrow_mut()
            .as_mut()
            .map(|r| r.install_scene(gpu_scene))
            .unwrap_or(false);

        // A scene read from an autosave or an earlier version stands for the
        // file it came from (#18).
        let recovered = crate::autosave::recovered_identity(&source_path);
        // A file the person opened goes first in the recent list (#381); a
        // recovered copy is not a file they opened.
        if recovered.is_none() {
            crate::recent_files::note(&source_path);
        }

        // Fires `SceneEvent::Installed`; the listener resets the camera to
        // `None` (so the bounds-based framing below sees a fresh slot), pushes
        // the new object list, and refreshes the other UI mirrors.
        self.session
            .document()
            .install_spatial(spatial, source_path, project_payload);

        // A reopened `.bread`'s rebuilt supports are the editable sets of
        // the fresh scene; any other load leaves no session in force.
        install_sets(self.session.document(), restored_supports.sets);
        // And the models it remembers hollowing; any other load, none.
        for why in &restored_hollows.failures {
            crate::breadcrumbs::note(&format!("load: hollow not restored: {why}"));
        }
        install_hollows(self.session.document(), restored_hollows.hollows);
        if let Some((source, origin)) = recovered {
            self.session.document().restore_identity(source, origin);
            self.mirror_title_to_ui();
        }

        // A reopened project's revalidated slices become the held result now,
        // after install has cleared any previous one, so the viewer opens on
        // them without a re-slice. The fresh scene's revision is the in-sync
        // baseline the write guard compares against.
        if let Some(reloaded) = reloaded_slices {
            let geometry = self.session.document().scene_revision();
            self.session
                .document()
                .set_slice_result(reloaded.into_slice_result(geometry));
        }

        // A freshly loaded scene opens in Transform, whose inspector leads
        // with the repair card unless every mesh is a clean manifold. Set
        // after install so the tabs, gated on a scene being present, are
        // already showing.
        self.set_section(WorkspaceSection::Transform);
        let document = self.session.document();
        let check = document.mesh_check();
        check.forget_all();
        for (mesh, sliceable) in &mesh_verdicts {
            check.remember(mesh, *sliceable, None);
        }
        if let (Some(ui), Some(Some(card))) = (
            self.ui.upgrade(),
            document.with_scene(|spatial| check.card(spatial.graph(), false)),
        ) {
            show_card(&ui, &card);
        }

        if scene_installed {
            self.session.loader().note_upload_success();
            // Frame on the union of the scene's world AABB and the build
            // envelope, so the plate is always inside the view and the orbit
            // centre and the zoom limits are anchored to the printer's
            // coordinate frame regardless of how big or small the loaded model
            // is. Mirrors the framing the pre-first-paint path uses when the
            // camera slot is empty.
            let scene_bounds = self
                .render
                .renderer_cell()
                .borrow()
                .as_ref()
                .and_then(|r| r.scene_bounds());
            self.camera
                .replace_with_framing(scene_bounds, self.session.build_volume());
            self.render.update_frame(FrameState::with_redraw_owed);
        } else {
            self.render.update_frame(FrameState::with_upload_owed);
        }
        // The models opened together with it (#391) are selected, and the
        // files that could not be read or do not fit are named.
        if !joined.models.is_empty() {
            document.set_selection(Selection::of(joined.models.iter().copied().map(LeafId)));
        }
        self.report_import_failures(&joined.failures);
        self.report_left_off(joined.left_off);
    }

    /// Returns the selection as its pre-order leaf index in the live graph.
    /// Test-only; the document's authoritative form is
    /// [`SceneDocument::selection_primary_node_id`].
    #[cfg(test)]
    pub(crate) fn selection(&self) -> Option<usize> {
        let document = self.session.document();
        document
            .selection_primary_node_id()
            .and_then(|id| document.with_scene(|s| s.leaf_index_of(id)).flatten())
    }

    /// Handles the Slint selection-changed callback. An out-of-bounds index
    /// maps to `None`: the UI should not hand one over, but a stale callback
    /// can race with a scene reload. The supports' row selects nothing
    /// outside the Supports tab (#120), and the list shows the selection it
    /// had again.
    pub(crate) fn on_select_by_index(&self, idx: usize) {
        self.on_modal_transform_finish(false);
        let supports_selectable = self.supports_selectable();
        let document = self.session.document();
        // `leaf_id_at_index` returns `None` past the last reachable leaf.
        let new_sel = document
            .with_scene(|spatial| {
                let id = spatial.leaf_id_at_index(idx);
                if !supports_selectable && id.is_some_and(|id| shows_supports(spatial, id)) {
                    return None;
                }
                Some(id.map_or(Selection::None, |id| Selection::One(LeafId(id))))
            })
            .unwrap_or(Some(Selection::None));
        let Some(new_sel) = new_sel else {
            self.mirror_selection_to_ui();
            return;
        };
        // `set_selection` short-circuits on no-change and fires
        // `SceneEvent::SelectionChanged` only on actual transitions; the
        // listener mirrors the selection to the UI and calls `mark_dirty`.
        document.set_selection(new_sel);
    }

    /// Handles Shift+click or Ctrl+click on a row of the object list (#13):
    /// the row joins the selection, or leaves it when it was in it, as
    /// Shift+click does in the viewport. The supports' row does nothing
    /// outside the Supports tab (#120).
    pub(crate) fn on_toggle_by_index(&self, idx: usize) {
        self.on_modal_transform_finish(false);
        let supports_selectable = self.supports_selectable();
        let document = self.session.document();
        let leaf = document
            .with_scene(|spatial| {
                let id = spatial.leaf_id_at_index(idx)?;
                (supports_selectable || !shows_supports(spatial, id)).then_some(id)
            })
            .flatten();
        match leaf {
            Some(id) => self.toggle_selected(id),
            None => self.mirror_selection_to_ui(),
        }
    }

    /// Adds `id` to the selection, or takes it out when it is in it.
    fn toggle_selected(&self, id: sc_scene::NodeId) {
        let document = self.session.document();
        let next = document.selection().toggled(LeafId(id));
        if next == Selection::None {
            self.input.gizmo_fsm().clear_hover();
        }
        document.set_selection(next);
    }

    /// Whether the supports can be selected: only in the Supports tab, which
    /// edits them. Anywhere else they follow their model (#120).
    fn supports_selectable(&self) -> bool {
        self.session.section() == WorkspaceSection::Supports
    }

    /// Cycles the selection for Tab and Shift+Tab.
    ///
    /// - `delta == 1` advances forward (Tab); `delta == -1` advances backward
    ///   (Shift+Tab). Other deltas count by their sign, so a stale Slint
    ///   binding cannot drive larger jumps, and a delta of 0 keeps an existing
    ///   selection.
    /// - With no selection, either direction selects the first leaf, so the
    ///   same key serves "select the first thing" and "advance to the next".
    /// - With a selection, the index wraps modulo the leaf count.
    /// - Outside the Supports tab the supports are skipped (#120).
    /// - On an empty or missing scene, nothing happens.
    ///
    /// Tab does not abort an in-progress drag.
    pub(crate) fn on_selection_cycle(&self, delta: i32) {
        let scene_len = self
            .session
            .document()
            .with_scene(|s| s.graph().reachable_leaf_count())
            .unwrap_or(0);
        if scene_len == 0 {
            return;
        }
        let step = delta.signum();
        let document = self.session.document();
        let cur_idx = document
            .selection_primary_node_id()
            .and_then(|id| document.with_scene(|s| s.leaf_index_of(id)).flatten());
        let len_i = scene_len as i32;
        let (first, step) = match cur_idx {
            None => (0, if step == 0 { 1 } else { step }),
            Some(cur) => ((cur as i32 + step).rem_euclid(len_i), step),
        };
        let supports_selectable = self.supports_selectable();
        let next = document
            .with_scene(|spatial| {
                (0..len_i)
                    .map(|k| (first + k * step).rem_euclid(len_i) as usize)
                    .find(|&i| {
                        supports_selectable
                            || spatial
                                .leaf_id_at_index(i)
                                .is_none_or(|id| !shows_supports(spatial, id))
                    })
            })
            .flatten();
        if let Some(next) = next {
            self.on_select_by_index(next);
        }
    }

    /// Handles Edit -> Undo. An empty stack is a no-op: the menu's `can-undo`
    /// gate can race a freshly emptied stack, so the document's own check
    /// decides.
    pub(crate) fn on_undo_requested(&self) {
        if self.input.modal.get().is_some() {
            self.on_modal_transform_finish(false);
            return;
        }
        if !self.allows_undo_redo() || self.repair_in_flight() {
            return;
        }
        if let Err(error) = self.session.document().undo() {
            self.show_edit_rejected(error);
        }
    }

    /// Handles Edit -> Redo.
    pub(crate) fn on_redo_requested(&self) {
        if self.input.modal.get().is_some() {
            self.on_modal_transform_finish(false);
            return;
        }
        if !self.allows_undo_redo() || self.repair_in_flight() {
            return;
        }
        if let Err(error) = self.session.document().redo() {
            self.show_edit_rejected(error);
        }
    }

    /// Shows a scene-rejected edit in the error dialog. The GUI builds its
    /// edits against the live scene, so a rejection indicates a programming
    /// bug, which the dialog makes visible.
    fn show_edit_rejected(&self, error: SceneError) {
        self.session.dialogs().show_error(move || ErrorTexts {
            heading: ctr!("ErrorDialog", "Edit failed"),
            context: None,
            message: ctr!(
                "ErrorDialog",
                "The scene rejected the last edit; nothing was changed."
            ),
            details: error.to_string().into(),
        });
    }

    /// Returns whether a repair worker is running. Undo and redo are silenced
    /// for the duration: an edit landing mid-run would invalidate the result
    /// the worker is computing, and its revision guard would then discard the
    /// whole run.
    fn repair_in_flight(&self) -> bool {
        self.ui
            .upgrade()
            .is_some_and(|ui| ui.get_repairing_in_progress())
    }

    /// Handles Delete from the viewport Delete key, the right-click context
    /// menu and the Edit > Delete menu item. No-op when no scene is loaded,
    /// nothing is selected, or the selection is not a leaf.
    pub(crate) fn on_delete_requested(&self) {
        if self.input.modal.get().is_some() {
            return;
        }
        // In the Supports tab, Delete takes the selected supports alone.
        if self.session.section() == WorkspaceSection::Supports {
            if let Err(error) = self.session.document().delete_selected_supports() {
                self.show_edit_rejected(error);
            }
            return;
        }
        if !self.allows_layout_tools() {
            return;
        }
        match self.session.document().delete_selected_leaf() {
            Ok(true) if !self.holds_objects() => {
                let emptied = self.on_emptied.borrow().clone();
                if let Some(emptied) = emptied {
                    emptied();
                }
            }
            Ok(_) => {}
            Err(error) => self.show_edit_rejected(error),
        }
    }

    /// Sets what runs when Delete takes the scene's last object (#397).
    pub(crate) fn set_on_emptied(&self, emptied: Rc<dyn Fn()>) {
        *self.on_emptied.borrow_mut() = Some(emptied);
    }

    /// Whether the window's scene holds at least 1 object.
    pub(crate) fn holds_objects(&self) -> bool {
        self.session
            .document()
            .with_scene(|scene| scene.leaf_id_at_index(0).is_some())
            .unwrap_or(false)
    }

    /// Handles Esc and a click on empty space. Each call drops 1 level of the
    /// gesture stack:
    ///
    /// 1. If a gizmo drag is in progress -> abort it (transform reverts to the
    ///    captured original), keep the selection and the focus.
    /// 2. Else if keyboard gizmo focus is active -> drop focus only, keep
    ///    selection.
    /// 3. Else -> clear the selection.
    ///
    /// Esc starts at level 1 and cascades through repeated presses. A click on
    /// empty space comes here from `on_pointer_up` after no object was hit;
    /// `on_pointer_up` ends any drag first, so a click starts at level 2.
    pub(crate) fn on_clear_selection(&self) {
        if self.input.modal.get().is_some() {
            self.on_modal_transform_finish(false);
            return;
        }
        if matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. }) {
            self.input.abort_gizmo_drag(self.session.document());
            return;
        }
        if self.input.kbd_drive().current_focus() != KbdFocus::None {
            self.apply_keyboard_focus(KbdFocus::None);
            return;
        }
        if matches!(self.session.document().selection(), Selection::None) {
            return;
        }
        // Clearing the selection hides the gizmo, so any cached hover tint
        // targets a handle that is not drawn: it is cleared first, then
        // `set_selection` fires `SelectionChanged` and the listener mirrors the
        // UI and calls `mark_dirty`.
        self.input.gizmo_fsm().clear_hover();
        self.session.document().set_selection(Selection::None);
    }

    fn mirror_modal_to_ui(&self) {
        // The gesture aids follow the modal state on the next frame.
        self.readout_owed.set(true);
        self.render.request_redraw();
        if let Some(ui) = self.ui.upgrade() {
            let gesture = self.input.modal.get();
            ui.set_modal_transform_active(gesture.is_some());
            let several = self.input.modal_group.borrow().is_some();
            let label = gesture
                .map(|g| {
                    let axis = g.axis.map(|i| ["X", "Y", "Z"][i]).unwrap_or(match g.op {
                        ModalOp::Scale => "Uniform",
                        _ => "View",
                    });
                    if !several || g.op == ModalOp::Move {
                        return format!("{} | {}", g.op.key(), axis);
                    }
                    // Several objects: which pivot they turn or scale about,
                    // and that the key again switches it (#218).
                    let key = g.op.key();
                    if g.individual {
                        format!(
                            "{key} {key} | {axis} | {}",
                            ctr!("MeshViewer", "Each object's own pivot ({}: shared)", key)
                        )
                    } else {
                        format!(
                            "{key} | {axis} | {}",
                            ctr!("MeshViewer", "Shared pivot ({}: each object's own)", key)
                        )
                    }
                })
                .unwrap_or_default();
            ui.set_modal_transform_status(label.into());
        }
    }

    pub(crate) fn on_modal_transform_start(&self, code: i32, x: f32, y: f32) {
        let Some(op) = ModalOp::from_code(code) else {
            return;
        };
        if !self.allows_transform_gizmo()
            || self.cut_on()
            || self.repair_in_flight()
            || self.ui.upgrade().is_some_and(|ui| ui.get_loading())
            || matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. })
        {
            return;
        }
        if let Some(mut gesture) = self.input.modal.get()
            && gesture.op == op
        {
            // R R and S S with several objects selected: each turns or scales
            // about its own pivot, and the key again goes back (#218).
            if op != ModalOp::Move && self.input.modal_group.borrow().is_some() {
                gesture.individual = !gesture.individual;
                self.input.modal.set(Some(gesture));
                self.mirror_modal_to_ui();
                self.update_modal(ScreenPoint::new(x, y), gesture.modifiers);
            }
            return;
        }
        self.on_modal_transform_finish(false);
        self.apply_keyboard_focus(KbdFocus::None);
        let Some(viewport) = self.render.viewport() else {
            return;
        };
        let document = self.session.document();
        let Some(node_id) = document.selection_primary_node_id() else {
            return;
        };
        // Several objects move, turn and scale together about their shared
        // centre (#13, #218).
        let group = document.selection_node_ids();
        let group_move = if group.len() > 1 {
            let Some(group_move) = GroupMove::capture(document, &group) else {
                return;
            };
            Some(Rc::new(group_move))
        } else {
            None
        };
        let gesture = document
            .with_scene(|spatial| {
                let camera_slot = self.camera.camera_cell().borrow();
                let camera = camera_slot.as_ref()?;
                let query = spatial.leaf(node_id)?;
                let pivot = super::gizmo_fsm::gizmo_pivot(spatial, node_id, &group)?;
                let fov = crate::mesh_render::default_fov_y();
                let screen = picking::project_world_to_screen(
                    camera,
                    fov,
                    viewport.width(),
                    viewport.height(),
                    pivot,
                )?;
                let inverse_view = camera.view_matrix().inverse();
                let view = inverse_view.transform_vector3(glam::Vec3::Z).normalize();
                // From an aligned view, G and R without an axis match it
                // exactly: G keeps the depth axis (#424), R turns about the
                // world axis at the camera (#351).
                let (right, up) = super::modal_transform::snap_view_plane(
                    inverse_view.transform_vector3(glam::Vec3::X).normalize(),
                    inverse_view.transform_vector3(glam::Vec3::Y).normalize(),
                    view,
                );
                Some(ModalGesture {
                    node_id,
                    original: query.local_transform(),
                    parent: query.parent_world(),
                    pivot,
                    start: [x, y],
                    cursor: [x, y],
                    screen_pivot: [screen.0, screen.1],
                    right,
                    up,
                    view_axis: super::modal_transform::snap_view_axis(view),
                    world_per_pixel: camera.world_per_pixel_at(pivot, fov, viewport.height())?,
                    op,
                    axis: None,
                    last_angle: (y - screen.1).atan2(x - screen.0),
                    angle: 0.0,
                    precision: gizmo::modifiers::PrecisionCursor::new([x, y], false),
                    modifiers: Default::default(),
                    factor: 1.0,
                    individual: false,
                })
            })
            .flatten();
        self.input.modal.set(gesture);
        self.input.modal_group.replace(gesture.and(group_move));
        self.input
            .modal_revision
            .set(gesture.map(|_| document.scene_revision()));
        self.input.gizmo_fsm().clear_hover();
        self.mirror_modal_to_ui();
        self.mark_dirty();
    }

    pub(crate) fn on_modal_transform_axis(&self, axis: i32, x: f32, y: f32) {
        if !(0..3).contains(&axis) {
            return;
        }
        let Some(mut gesture) = self.input.modal.get() else {
            return;
        };
        gesture.axis = Some(axis as usize);
        self.input.modal.set(Some(gesture));
        self.mirror_modal_to_ui();
        self.update_modal(ScreenPoint::new(x, y), gesture.modifiers);
    }

    fn update_modal(&self, cursor: ScreenPoint, modifiers: gizmo::modifiers::DragModifiers) {
        if self.input.modal_revision.get() != Some(self.session.document().scene_revision()) {
            self.input.modal.set(None);
            self.input.modal_revision.set(None);
            self.input.modal_group.replace(None);
            self.mirror_modal_to_ui();
            return;
        }
        let Some(mut gesture) = self.input.modal.get() else {
            return;
        };
        let Some(proposed) = gesture.propose([cursor.x, cursor.y], modifiers) else {
            return;
        };
        let group = self.input.modal_group.borrow().clone();
        if let Some(group) = group {
            self.input.modal.set(Some(gesture));
            self.update_modal_group(&gesture, &group, proposed);
            return;
        }
        let result = self
            .session
            .document()
            .with_scene(|spatial| {
                let query = spatial.leaf(gesture.node_id)?;
                let before = query.local_transform();
                let after = match gesture.op {
                    ModalOp::Move if gesture.axis == Some(2) => proposed,
                    ModalOp::Move => {
                        let baseline =
                            query.minimum_world_z(gesture.parent.compose(&gesture.original))?;
                        let next = query.minimum_world_z(gesture.parent.compose(&proposed))?;
                        let lift = (baseline.min(0.0) - next).max(0.0);
                        gesture
                            .world_translation(proposed, glam::Vec3::new(0.0, 0.0, lift as f32))?
                    }
                    _ => super::bed_clearance::keep_above_bed(
                        spatial,
                        gesture.node_id,
                        gesture.original,
                        proposed,
                    ),
                };
                Some((before, after))
            })
            .flatten();
        let Some((before, after)) = result else {
            self.on_modal_transform_finish(false);
            return;
        };
        self.input.modal.set(Some(gesture));
        if let Err(error) =
            self.session
                .document()
                .apply_edit(sc_scene_edit::SceneEdit::SetLocalTransform {
                    node_id: gesture.node_id,
                    before,
                    after,
                })
        {
            self.on_modal_transform_finish(false);
            if let EditNotApplied::Rejected(reason) = error {
                self.show_edit_rejected(reason);
            }
        }
        self.mark_dirty();
    }

    /// One step of `G`, `R` or `S` with several objects selected: each gets
    /// the world transform `proposed` gives the primary one about their
    /// shared centre. A move is lifted as one so that none sinks below the
    /// plate (unless it is along Z); a turn or a scale keeps the group on the
    /// plate as one object is kept.
    fn update_modal_group(&self, gesture: &ModalGesture, group: &GroupMove, proposed: Transform3D) {
        let document = self.session.document();
        let each = gesture.individual.then(|| gesture.linear_delta()).flatten();
        let applied = if let Some(m) = each {
            group.apply_each(document, &m)
        } else if gesture.op == ModalOp::Move {
            let at = |t: &Transform3D| match gesture.parent.compose(t) {
                Transform3D::Identity => [0.0; 3],
                Transform3D::Affine(a) => *a.translation(),
            };
            let (a, b) = (at(&gesture.original), at(&proposed));
            let mut d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            if gesture.axis != Some(2) {
                d[2] += group.floor_lift(document, d);
            }
            group.apply(document, d)
        } else {
            let Some(delta) = super::group_move::world_delta(
                &gesture.parent.compose(&gesture.original),
                &gesture.parent.compose(&proposed),
            ) else {
                return;
            };
            group.apply_world(document, group.kept_on_bed(document, delta))
        };
        if let Err(error) = applied {
            self.on_modal_transform_finish(false);
            if let EditNotApplied::Rejected(reason) = error {
                self.show_edit_rejected(reason);
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn on_modal_transform_finish(&self, confirm: bool) {
        let Some(gesture) = self.input.modal.take() else {
            return;
        };
        let group = self.input.modal_group.take();
        let document = self.session.document();
        if self.input.modal_revision.take() != Some(document.scene_revision()) {
            // A new document or externally committed edit owns the scene now.
            // Never restore a stale snapshot onto a possibly reused node id.
            self.mirror_modal_to_ui();
            self.mark_dirty();
            return;
        }
        if let Some(group) = group {
            if confirm {
                group.commit(document);
            } else {
                group.abort(document);
            }
            self.mirror_modal_to_ui();
            self.mark_dirty();
            return;
        }
        let current = document
            .with_scene(|s| s.leaf(gesture.node_id).map(|q| q.local_transform()))
            .flatten();
        if let Some(current) = current {
            if confirm {
                document.push_edit(sc_scene_edit::SceneEdit::SetLocalTransform {
                    node_id: gesture.node_id,
                    before: gesture.original,
                    after: current,
                });
            } else {
                let _ = document.apply_edit(sc_scene_edit::SceneEdit::SetLocalTransform {
                    node_id: gesture.node_id,
                    before: current,
                    after: gesture.original,
                });
            }
        }
        self.mirror_modal_to_ui();
        self.mark_dirty();
    }

    /// Applies the gizmo style and size chosen in Preferences (#274, #276):
    /// `size` in percent.
    pub(crate) fn set_gizmo_prefs(&self, style: gizmo::look::GizmoStyle, size: u32) {
        let look = gizmo::look::GizmoLook {
            style,
            size: gizmo::look::clamp_size(size) as f32 / 100.0,
            ..self.input.gizmo_look()
        };
        if self.input.set_gizmo_look(look) {
            self.mirror_gizmo_tool_to_ui();
            self.mark_dirty();
        }
    }

    /// The rail's tool (Move, Rotate, Scale): with the focused style, which
    /// handles the gizmo shows (#274).
    fn set_gizmo_tool(&self, tool: gizmo::look::GizmoTool) {
        let look = gizmo::look::GizmoLook {
            tool,
            ..self.input.gizmo_look()
        };
        if self.input.set_gizmo_look(look) {
            self.mirror_gizmo_tool_to_ui();
            self.mark_dirty();
        }
    }

    fn mirror_gizmo_tool_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let look = self.input.gizmo_look();
            ui.set_gizmo_focused_style(look.style == gizmo::look::GizmoStyle::Focused);
            ui.set_gizmo_tool(look.tool.index());
        }
    }

    #[cfg(test)]
    pub(crate) fn gizmo_look_for_test(&self) -> gizmo::look::GizmoLook {
        self.input.gizmo_look()
    }

    #[cfg(test)]
    pub(super) fn gizmo_state_for_test(&self) -> GizmoState {
        self.input.gizmo_fsm().current()
    }

    #[cfg(test)]
    pub(super) fn gizmo_hover_for_test(&self) -> Option<crate::mesh_render::GizmoHover> {
        self.input.gizmo_fsm().current_hover()
    }

    /// Arms the translate keyboard focus (`T`); `R` and `S` arm rotate and
    /// scale the same way. Each needs a selection; without one, the press does
    /// nothing. A press of the focus already armed does nothing, so autorepeat
    /// does not reset the integrator's tick clock.
    pub(crate) fn on_gizmo_focus_translate(&self) {
        self.on_modal_transform_finish(false);
        self.set_gizmo_tool(gizmo::look::GizmoTool::Move);
        self.apply_keyboard_focus(KbdFocus::Translate);
    }

    pub(crate) fn on_gizmo_focus_rotate(&self) {
        self.set_gizmo_tool(gizmo::look::GizmoTool::Rotate);
        self.apply_keyboard_focus(KbdFocus::Rotate);
    }

    pub(crate) fn on_gizmo_focus_scale(&self) {
        self.set_gizmo_tool(gizmo::look::GizmoTool::Scale);
        self.apply_keyboard_focus(KbdFocus::Scale);
    }

    /// Sets the keyboard focus through `InputDispatch` and, on an actual
    /// change, mirrors the activation state to the UI and calls `mark_dirty`.
    /// The mirroring happens here because `InputDispatch` owns the focus cell
    /// and the coordinator owns the UI handle.
    fn apply_keyboard_focus(&self, focus: KbdFocus) {
        // Arming a focus (Translate, Rotate or Scale) is a transform tool and
        // is gated to editing sections; dropping focus (`None`) must
        // always work so entering Slice can clear it.
        if focus != KbdFocus::None && (!self.allows_transform_gizmo() || self.cut_on()) {
            return;
        }
        let Some(new_focus) = self
            .input
            .set_keyboard_focus(self.session.document(), focus)
        else {
            return;
        };
        if let Some(ui) = self.ui.upgrade() {
            ui.set_gizmo_keyboard_focus_active(new_focus != KbdFocus::None);
        }
        self.mark_dirty();
    }

    pub(crate) fn on_gizmo_arrow_pressed(&self, code: i32) {
        if self
            .input
            .on_gizmo_arrow_pressed(self.session.document(), code)
        {
            self.mark_dirty();
        }
    }

    pub(crate) fn on_gizmo_arrow_released(&self, code: i32) {
        if self
            .input
            .on_gizmo_arrow_released(self.session.document(), code)
        {
            self.mark_dirty();
        }
    }

    pub(crate) fn on_pointer_down(&self, cursor: ScreenPoint, mut modifiers: PointerModifiers) {
        if self.lasso_down(cursor) || self.brush_down(cursor, &modifiers) || self.cut_down(cursor) {
            return;
        }
        // Shift+click with Add armed hangs the support from the nearest one
        // (#3): the press is the tool's, not the start of a selection box.
        if modifiers.shift && self.add_support_armed() {
            modifiers.shift = false;
        }
        // A press of a drag tool (Move tip; Remove's box or brush, Shift
        // keeping its selection box) is a click until it moves.
        let drag = self
            .drag_tool_armed()
            .filter(|kind| !(modifiers.shift && *kind != SupportDragKind::MoveTip));
        self.support_drag.set(match drag {
            Some(kind) => DragPhase::Pressed([cursor.x, cursor.y], kind),
            None => DragPhase::Idle,
        });
        let boxing = self.input.on_pointer_down(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
            modifiers,
            self.allows_transform_gizmo() && !self.cut_on(),
            self.supports_selectable(),
        );
        if let Some(ui) = self.ui.upgrade() {
            ui.set_box_select_armed(boxing);
        }
    }

    pub(crate) fn on_pointer_move(&self, cursor: ScreenPoint, modifiers: PointerModifiers) {
        // Track the cursor in every section so a later paste anchors on it; the
        // gizmo drag and hover run in Prepare and Transform.
        self.input.note_hover_cursor(cursor);
        if self.lasso_move(cursor) || self.brush_move(cursor, &modifiers) {
            return;
        }
        if self.support_drag_move(cursor) || self.cut_move(cursor) {
            return;
        }
        self.tab_tool_hover(cursor, modifiers.shift);
        if self.input.modal.get().is_some() {
            self.update_modal(cursor, super::drag_modifiers(modifiers));
            return;
        }
        if !self.allows_transform_gizmo() || self.cut_on() {
            return;
        }
        if self.input.on_pointer_move(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
            modifiers,
        ) {
            self.mark_dirty();
        }
    }

    /// Ends a left press: commits a gizmo drag, or picks on a click (with
    /// Shift, the object joins the selection or leaves it, #13), or selects
    /// what a Shift drag's box touches, on top of the selection.
    pub(crate) fn on_pointer_up(&self, cursor: ScreenPoint, modifiers: PointerModifiers) {
        if self.lasso_up(cursor) || self.brush_up() || self.cut_up(cursor) {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_box_select_armed(false);
        }
        let outcome = self.input.on_pointer_up(self.session.document(), cursor);
        match self.support_drag.replace(DragPhase::Idle) {
            DragPhase::Dragging(_) => {
                if let Some(handler) = self.support_drag_handler() {
                    handler.drag_end(true);
                }
                self.mark_dirty();
                return;
            }
            DragPhase::Ignored => return,
            DragPhase::Idle | DragPhase::Pressed(..) => {}
        }
        match outcome {
            PointerUpOutcome::DragCommitted => self.mark_dirty(),
            PointerUpOutcome::Click(cursor) => {
                let forced = modifiers.shift && self.add_support_armed();
                if modifiers.shift && !forced {
                    self.run_toggle_pick_at(cursor);
                } else if self.cut_click(cursor) {
                    self.mark_dirty();
                } else if !self.support_tool_click(cursor, forced) {
                    self.run_pick_at(cursor);
                }
            }
            PointerUpOutcome::BoxSelect { from, to } => self.select_in_box(from, to),
            PointerUpOutcome::Nothing => (),
        }
    }

    /// Shift+click: the object under `cursor` joins the selection, or leaves
    /// it when it was in it. Empty space keeps the selection as it is.
    fn run_toggle_pick_at(&self, cursor: ScreenPoint) {
        let pick = self.input.pick_at(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
            self.supports_selectable(),
        );
        if let Some(Some(node_id)) = pick {
            self.toggle_selected(node_id);
        }
    }

    /// Adds every object the box from `from` to `to` touches on screen to
    /// the selection (see [`super::box_select`]). Outside the Supports tab,
    /// supports in the box bring their model instead (#120).
    fn select_in_box(&self, from: ScreenPoint, to: ScreenPoint) {
        let Some(viewport) = self.render.viewport() else {
            return;
        };
        let document = self.session.document();
        let supports_selectable = self.supports_selectable();
        let sessions = document.support_sessions();
        let touched = document
            .with_scene(|spatial| {
                let camera_slot = self.camera.camera_cell().borrow();
                let camera = camera_slot.as_ref()?;
                Some(super::box_select::leaves_touching(
                    spatial,
                    camera,
                    crate::mesh_render::default_fov_y(),
                    [viewport.width(), viewport.height()],
                    super::box_select::ScreenRect::spanning([from.x, from.y], [to.x, to.y]),
                    |id| {
                        if supports_selectable || !shows_supports(spatial, id) {
                            Some(id)
                        } else {
                            crate::support_persist::model_of(&sessions, id)
                                .filter(|&model| spatial.leaf(model).is_some())
                        }
                    },
                ))
            })
            .flatten()
            .unwrap_or_default();
        crate::breadcrumbs::note(&format!("box select: {} object(s)", touched.len()));
        if touched.is_empty() {
            return;
        }
        let next = document
            .selection()
            .with_added(touched.into_iter().map(LeafId));
        document.set_selection(next);
    }

    /// Hands the Supports tab's drags to `handler` (the support driver).
    pub(crate) fn set_support_drag_handler(&self, handler: std::rc::Weak<dyn SupportDragHandler>) {
        *self.support_drag_handler.borrow_mut() = Some(handler);
    }

    fn support_drag_handler(&self) -> Option<Rc<dyn SupportDragHandler>> {
        self.support_drag_handler.borrow().as_ref()?.upgrade()
    }

    /// The drag a press makes now, if a drag tool is armed.
    fn drag_tool_armed(&self) -> Option<SupportDragKind> {
        #[cfg(test)]
        if let Some(kind) = self.drag_tool_for_test.get() {
            return Some(kind);
        }
        self.ui.upgrade().and_then(|ui| self.support_drag_kind(&ui))
    }

    /// Arms a drag tool, for tests that run without a window.
    #[cfg(test)]
    pub(crate) fn arm_drag_tool_for_test(&self, kind: Option<SupportDragKind>) {
        self.drag_tool_for_test.set(kind);
    }

    /// The drag a press makes with the tool armed: Move tip's, or Remove's
    /// box or brush.
    fn support_drag_kind(&self, ui: &MainWindow) -> Option<SupportDragKind> {
        match self.tab_tool(ui)? {
            TabTool::MoveTip => Some(SupportDragKind::MoveTip),
            TabTool::RemoveSupport if ui.get_support_remove_mode() == 1 => {
                Some(SupportDragKind::Brush)
            }
            TabTool::RemoveSupport => Some(SupportDragKind::Box),
            TabTool::AddSupport | TabTool::Brace | TabTool::Drain => None,
        }
    }

    /// What lies under `cursor` for a drag: the model, looking through the
    /// supports, and the supports.
    fn drag_point(&self, cursor: ScreenPoint) -> DragPoint {
        let at = |tool| {
            self.tab_tool_hit(tool, cursor)
                .map(|(_, p, _)| [p.x, p.y, p.z])
        };
        DragPoint {
            cursor: [cursor.x, cursor.y],
            model: at(TabTool::AddSupport),
            support: at(TabTool::RemoveSupport),
        }
    }

    /// Runs `with` with where world points land on the viewport now.
    fn with_projection<R>(
        &self,
        with: impl FnOnce(super::support_drag::Project<'_>) -> R,
    ) -> Option<R> {
        let viewport = self.render.viewport()?;
        let fov = crate::mesh_render::default_fov_y();
        let (projector, camera) = {
            let camera_slot = self.camera.camera_cell().borrow();
            let camera = camera_slot.as_ref()?.clone();
            let projector = super::box_select::Projector::new(
                &camera,
                fov,
                viewport.width(),
                viewport.height(),
            );
            (projector, camera)
        };
        let project = |p: [f32; 3]| projector.project(glam::Vec3::from(p));
        let (width, height) = (viewport.width(), viewport.height());
        // What the camera sees first through each pixel: a ray from it, the
        // nearest hit among every leaf, model or supports. A ray is about a
        // millisecond on a big model and a box asks hundreds: they share
        // the cores.
        let document = self.session.document();
        let sessions = document.support_sessions();
        let through_rafts = self.sees_past_rafts(&camera);
        let hits = |pixels: &[[f32; 2]]| -> Vec<Option<super::support_drag::ScreenHit>> {
            document
                .with_scene(|spatial| {
                    // From under the plate the rafts are hidden: the
                    // camera sees past them (#481).
                    let raft_top = if through_rafts {
                        hidden_below_mm(crate::support_persist::raft_top_mm(spatial, &sessions))
                    } else {
                        0.0
                    };
                    let one = |at: &[f32; 2]| {
                        let ray =
                            picking::ray_from_pixel(&camera, fov, width, height, at[0], at[1]);
                        let hit = picking::pick_face_past_rafts(
                            &ray,
                            spatial,
                            |_| true,
                            |id| shows_supports(spatial, id),
                            raft_top,
                        )?;
                        Some(super::support_drag::ScreenHit {
                            leaf: hit.leaf,
                            point: (ray.origin + ray.direction * hit.t).into(),
                        })
                    };
                    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
                    let threads = cores.min(pixels.len().div_ceil(8)).max(1);
                    if threads == 1 {
                        return pixels.iter().map(one).collect();
                    }
                    let chunk = pixels.len().div_ceil(threads);
                    std::thread::scope(|scope| {
                        let parts: Vec<_> = pixels
                            .chunks(chunk)
                            .map(|part| {
                                scope.spawn(move || part.iter().map(one).collect::<Vec<_>>())
                            })
                            .collect();
                        parts
                            .into_iter()
                            .zip(pixels.chunks(chunk))
                            .flat_map(|(part, pixels)| {
                                part.join().unwrap_or_else(|_| vec![None; pixels.len()])
                            })
                            .collect()
                    })
                })
                .unwrap_or_else(|| vec![None; pixels.len()])
        };
        let view = super::support_drag::DragView {
            project: &project,
            hits: &hits,
        };
        Some(with(&view))
    }

    /// Moves a drag of a Supports tab tool: one past the click threshold
    /// starts, if the driver takes it. Returns whether the move was the
    /// drag's, so the hover leaves it alone.
    fn support_drag_move(&self, cursor: ScreenPoint) -> bool {
        match self.support_drag.get() {
            DragPhase::Idle => false,
            DragPhase::Ignored => true,
            DragPhase::Pressed(at, kind) => {
                let moved = (cursor.x - at[0]).hypot(cursor.y - at[1]);
                if moved <= super::input_state::CLICK_DRAG_THRESHOLD {
                    return false;
                }
                let Some(handler) = self.support_drag_handler() else {
                    self.support_drag.set(DragPhase::Ignored);
                    return true;
                };
                let press = self.drag_point(ScreenPoint { x: at[0], y: at[1] });
                let taken = self
                    .with_projection(|project| handler.drag_start(kind, &press, project))
                    .unwrap_or(false);
                if !taken {
                    self.support_drag.set(DragPhase::Ignored);
                    return true;
                }
                self.support_drag.set(DragPhase::Dragging(kind));
                if kind == SupportDragKind::Box
                    && let Some(ui) = self.ui.upgrade()
                {
                    ui.set_box_select_armed(true);
                }
                self.support_drag_move(cursor)
            }
            DragPhase::Dragging(_) => {
                if let Some(handler) = self.support_drag_handler() {
                    let at = self.drag_point(cursor);
                    self.with_projection(|project| handler.drag_move(&at, project));
                }
                true
            }
        }
    }

    /// Drops a drag of a Supports tab tool in flight (Esc, a right press, a
    /// tool put away): nothing of it is made; with none, what a tool holds
    /// between two clicks (the brace tool's first end, #3). Returns whether
    /// there was one.
    pub(crate) fn cancel_support_drag(&self) -> bool {
        let pending = || {
            self.support_drag_handler()
                .is_some_and(|handler| handler.cancel_pending())
        };
        match self.support_drag.get() {
            DragPhase::Dragging(_) => {
                self.support_drag.set(DragPhase::Ignored);
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_box_select_armed(false);
                }
                if let Some(handler) = self.support_drag_handler() {
                    handler.drag_end(false);
                }
                self.mark_dirty();
                true
            }
            DragPhase::Pressed(..) => {
                self.support_drag.set(DragPhase::Ignored);
                false
            }
            DragPhase::Idle | DragPhase::Ignored => pending(),
        }
    }

    /// Whether the Supports tab's Add tool is armed.
    fn add_support_armed(&self) -> bool {
        self.ui
            .upgrade()
            .is_some_and(|ui| self.tab_tool(&ui) == Some(TabTool::AddSupport))
    }

    /// The tab tool armed in the current section, if any.
    fn tab_tool(&self, ui: &MainWindow) -> Option<TabTool> {
        match self.session.section() {
            WorkspaceSection::Hollow if ui.get_hollow_drain_tool() => Some(TabTool::Drain),
            WorkspaceSection::Supports => match ui.get_support_tool() {
                1 => Some(TabTool::AddSupport),
                2 => Some(TabTool::RemoveSupport),
                3 => Some(TabTool::MoveTip),
                4 => Some(TabTool::Brace),
                _ => None,
            },
            _ => None,
        }
    }

    /// The world point under `cursor` a tab tool works on, and its leaf. The
    /// add and drain tools look through the support mesh to the model; the
    /// remove tool sees the support mesh alone, even behind the model.
    fn tab_tool_hit(
        &self,
        tool: TabTool,
        cursor: ScreenPoint,
    ) -> Option<(sc_scene::NodeId, glam::Vec3, glam::Vec3)> {
        let vp = self.render.viewport()?;
        let (ray, through_rafts) = {
            let camera_slot = self.camera.camera_cell().borrow();
            let camera = camera_slot.as_ref()?;
            let ray = picking::ray_from_pixel(
                camera,
                crate::mesh_render::default_fov_y(),
                vp.width(),
                vp.height(),
                cursor.x,
                cursor.y,
            );
            (ray, self.sees_past_rafts(camera))
        };
        let document = self.session.document();
        let sessions = document.support_sessions();
        document
            .with_scene(|spatial| {
                let on_supports = matches!(
                    tool,
                    TabTool::RemoveSupport | TabTool::MoveTip | TabTool::Brace
                );
                // Remove and Move tip take only a support in sight (#3): the
                // nearest hit among every leaf, model or supports, is one;
                // from under the plate, past the hidden rafts (#481).
                // Add and the drain look through the supports to the model.
                let hit = if on_supports {
                    let raft_top = if through_rafts {
                        hidden_below_mm(crate::support_persist::raft_top_mm(spatial, &sessions))
                    } else {
                        0.0
                    };
                    picking::pick_face_past_rafts(
                        &ray,
                        spatial,
                        |_| true,
                        |id| shows_supports(spatial, id),
                        raft_top,
                    )
                    .filter(|hit| shows_supports(spatial, hit.leaf))?
                } else {
                    picking::pick_face_where(&ray, spatial, |id| !shows_supports(spatial, id))?
                };
                let normal =
                    picking::face_normal_towards(&ray, &spatial.leaf(hit.leaf)?, hit.face as usize)
                        .unwrap_or(glam::Vec3::Z);
                Some((hit.leaf, ray.origin + ray.direction * hit.t, normal))
            })
            .flatten()
    }

    /// A click with a tab tool (the Supports tab's Add or Delete, the Hollow
    /// tab's drain): hands the world point under the cursor to its driver,
    /// with `forced` for a Shift+click of Add. The drain tool selects the
    /// model it hit first. Returns whether the click was the tool's (a miss
    /// with a tool on is still the tool's).
    fn support_tool_click(&self, cursor: ScreenPoint, forced: bool) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        let Some(tool) = self.tab_tool(&ui) else {
            return false;
        };
        // Move tip only drags: a click grabs nothing.
        if tool == TabTool::MoveTip {
            return true;
        }
        // The preview showed what this click does; the edit replaces it.
        self.set_preview_overlay(None);
        if let Some((id, p, n)) = self.tab_tool_hit(tool, cursor) {
            if tool == TabTool::Drain {
                self.session
                    .document()
                    .set_selection(Selection::One(LeafId(id)));
                ui.invoke_hollow_drain_clicked(p.x, p.y, p.z, n.x, n.y, n.z);
            } else {
                ui.invoke_support_tool_clicked(p.x, p.y, p.z, forced);
            }
            self.mark_dirty();
        }
        true
    }

    /// The cursor moved with no button down: an armed tab tool previews what
    /// a click there would do (its driver draws it, debounced), a Shift+click
    /// with `shift` held; off the model, or with no tool, the preview goes.
    fn tab_tool_hover(&self, cursor: ScreenPoint, shift: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(tool) = self.tab_tool(&ui) else {
            return;
        };
        let hit = self.tab_tool_hit(tool, cursor);
        let (p, n) = hit.map_or((glam::Vec3::ZERO, glam::Vec3::Z), |(_, p, n)| (p, n));
        if tool == TabTool::Drain {
            // A drain on another model selects it first: its own cavity
            // says whether the drain reaches it.
            let on_selected = hit.is_some_and(|(id, _, _)| {
                self.session.document().selection().primary().map(|l| l.0) == Some(id)
            });
            ui.invoke_hollow_drain_hover(p.x, p.y, p.z, n.x, n.y, n.z, hit.is_some(), on_selected);
        } else {
            ui.invoke_support_tool_hover(p.x, p.y, p.z, hit.is_some(), shift);
        }
    }

    /// Installs the brush that paints faces (#183).
    pub(crate) fn set_surface_brush(&self, brush: Rc<SurfaceBrush>) {
        *self.surface_brush.borrow_mut() = Some(brush);
    }

    /// Shows the paint on the models, or hides it. The ring goes with it.
    pub(crate) fn show_paint(&self, on: bool) {
        if self.paint_shown.replace(on) != on {
            self.paint_owed.set(true);
            self.mark_dirty();
        }
    }

    fn armed_brush(&self) -> Option<Rc<SurfaceBrush>> {
        self.surface_brush
            .borrow()
            .as_ref()
            .filter(|brush| brush.armed())
            .cloned()
    }

    /// Whether the brush is on: it takes the left button, and the gizmo goes.
    pub(crate) fn brush_on(&self) -> bool {
        self.armed_brush().is_some()
    }

    /// Where the pixel `at` lands on the model the brush works on.
    fn brush_aim(&self, at: [f32; 2]) -> Option<BrushAim> {
        let vp = self.render.viewport()?;
        let target = self.session.document().selection_primary_node_id()?;
        let (ray, view) = {
            let camera_slot = self.camera.camera_cell().borrow();
            let camera = camera_slot.as_ref()?;
            let ray = picking::ray_from_pixel(
                camera,
                crate::mesh_render::default_fov_y(),
                vp.width(),
                vp.height(),
                at[0],
                at[1],
            );
            let view = View {
                eye: camera.eye().to_array(),
                direction: ray.direction.to_array(),
                orthographic: camera.orthographic,
            };
            (ray, view)
        };
        self.session
            .document()
            .with_scene(|spatial| {
                aim(spatial, &ray, &view, target, |id| {
                    !shows_supports(spatial, id)
                })
            })
            .flatten()
    }

    /// A left press with the brush on: starts a stroke where it lands on the
    /// model, or fills with Shift. The press is the brush's anywhere in the
    /// view: it selects nothing and moves nothing.
    fn brush_down(&self, cursor: ScreenPoint, modifiers: &PointerModifiers) -> bool {
        let Some(brush) = self.armed_brush() else {
            return false;
        };
        self.brush_press.set(true);
        let at = [cursor.x, cursor.y];
        let erase = brush.erasing() != modifiers.alt;
        match self.brush_aim(at) {
            Some(aim) => {
                if modifiers.shift {
                    brush.fill(&aim, erase);
                } else {
                    brush.begin(&aim, at, erase);
                }
                self.mark_dirty();
            }
            None if !modifiers.shift => self.brush_waiting.set(Some(erase)),
            None => {}
        }
        true
    }

    /// A move with the brush on: a stroke goes on, and the ring follows the
    /// surface. False when the brush is off, or the move is another
    /// gesture's: only then does the move go further.
    fn brush_move(&self, cursor: ScreenPoint, modifiers: &PointerModifiers) -> bool {
        let Some(brush) = self.armed_brush() else {
            return false;
        };
        let at = [cursor.x, cursor.y];
        let aim = self.brush_aim(at);
        brush.hover(aim.as_ref(), brush.erasing() != modifiers.alt);
        if brush.stroking() {
            brush.extend(at, &|pixel| self.brush_aim(pixel));
        } else if let (Some(erase), Some(aim)) = (self.brush_waiting.get(), aim) {
            self.brush_waiting.set(None);
            brush.begin(&aim, at, erase);
        }
        self.mark_dirty();
        self.brush_press.get()
    }

    /// The left release that ends a brush press.
    fn brush_up(&self) -> bool {
        if !self.brush_press.replace(false) {
            return false;
        }
        self.brush_waiting.set(None);
        if let Some(brush) = self.surface_brush.borrow().as_ref() {
            brush.end(true);
        }
        self.mark_dirty();
        true
    }

    /// Installs the Cut tool's handles: offered the pointer in the
    /// Transform tab first while the tool is open (#56).
    pub(crate) fn set_cut_handles(&self, handles: Rc<dyn CutHandles>) {
        *self.cut_handles.borrow_mut() = Some(handles);
    }

    /// What stays lit while something is selected (#523), worked out again
    /// only when the scene or the selection changed.
    fn focus(&self) -> Vec<sc_scene::NodeId> {
        let document = self.session.document();
        let key = (document.scene_revision(), document.selection_node_ids());
        if let Some((at, focus)) = self.focus_cache.borrow().as_ref()
            && *at == key
        {
            return focus.clone();
        }
        let focus = document.focus_ids();
        *self.focus_cache.borrow_mut() = Some((key, focus.clone()));
        focus
    }

    /// The Cut tool's handles while it is open in the Transform tab: the
    /// object's gizmo then goes, and the object does not move.
    fn open_cut(&self) -> Option<Rc<dyn CutHandles>> {
        if self.session.section() != WorkspaceSection::Transform {
            return None;
        }
        self.cut_handles.borrow().clone().filter(|h| h.active())
    }

    pub(crate) fn cut_on(&self) -> bool {
        self.open_cut().is_some()
    }

    /// The pointer at `cursor`, as the Cut tool takes it: its ray, and how
    /// the camera puts world points on the viewport now.
    fn scene_pointer(&self, cursor: ScreenPoint) -> Option<ScenePointer> {
        let vp = self.render.viewport()?;
        let camera = self.camera.camera_cell().borrow().as_ref()?.clone();
        let fov = crate::mesh_render::default_fov_y();
        let (w, h) = (vp.width(), vp.height());
        let look = self.input.gizmo_look();
        let metrics = look.metrics();
        let sized = camera.clone();
        let ray = picking::ray_from_pixel(&camera, fov, w, h, cursor.x, cursor.y);
        let (o, d) = (ray.origin, ray.direction);
        Some(ScenePointer {
            origin: [f64::from(o.x), f64::from(o.y), f64::from(o.z)],
            direction: [f64::from(d.x), f64::from(d.y), f64::from(d.z)],
            cursor: [f64::from(cursor.x), f64::from(cursor.y)],
            project: Box::new(move |p: [f64; 3]| {
                let at = glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
                picking::project_world_to_screen(&camera, fov, w, h, at)
                    .map(|(x, y)| [f64::from(x), f64::from(y)])
            }),
            scale_at: Box::new(move |p: [f64; 3]| {
                let at = glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
                gizmo::gizmo_scale_sized(&sized, at, fov, h, look.size).map(f64::from)
            }),
            ring_radius: f64::from(metrics.ring_radius),
            arrow_tip: f64::from(metrics.cone_tip),
        })
    }

    /// The Cut tool's gizmo for this frame, in the object's gizmo's style
    /// and size.
    fn cut_gizmo_frame(
        &self,
        handles: &dyn CutHandles,
        camera: &crate::camera::OrbitCamera,
        viewport_h_logical: f32,
    ) -> Option<crate::mesh_render::GizmoFrame> {
        let (center, tool, turn) = handles.gizmo()?;
        let v3 = |p: [f64; 3]| glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
        let drag_visual = turn.map(|t| crate::mesh_render::GizmoDragVisual::ViewRotate {
            right: v3(t.right),
            up: v3(t.up),
            start: t.start as f32,
            sweep: t.sweep as f32,
        });
        let mut frame = gizmo::frame::build_gizmo_frame_at(
            v3(center),
            camera,
            viewport_h_logical,
            crate::mesh_render::GizmoVisibility::Idle,
            None,
            drag_visual,
            self.input.gizmo_look(),
        )?;
        frame.tool = Some(tool);
        Some(frame)
    }

    /// A left press with the Cut tool open: on its gizmo or plane, the
    /// press is the tool's.
    fn cut_down(&self, cursor: ScreenPoint) -> bool {
        let Some(handles) = self.open_cut() else {
            return false;
        };
        let Some(pointer) = self.scene_pointer(cursor) else {
            return false;
        };
        if !handles.press(&pointer) {
            return false;
        }
        self.cut_press.set(Some(([cursor.x, cursor.y], false)));
        self.mark_dirty();
        true
    }

    /// A move with the Cut tool open: drags what it took, or lights the
    /// handle under the cursor.
    fn cut_move(&self, cursor: ScreenPoint) -> bool {
        let Some(handles) = self.cut_handles.borrow().clone() else {
            return false;
        };
        let pointer = self.scene_pointer(cursor);
        match self.cut_press.get() {
            Some((from, moved)) => {
                let far = (cursor.x - from[0]).hypot(cursor.y - from[1]) > CUT_DRAG_PX;
                if moved || far {
                    self.cut_press.set(Some((from, true)));
                    if let Some(pointer) = pointer {
                        handles.drag(&pointer);
                    }
                    self.mark_dirty();
                }
                true
            }
            None => {
                if self.cut_on() && handles.hover(pointer.as_ref()) {
                    self.mark_dirty();
                }
                false
            }
        }
    }

    /// The release of a press the Cut tool took.
    fn cut_up(&self, cursor: ScreenPoint) -> bool {
        let Some((_, moved)) = self.cut_press.take() else {
            return false;
        };
        if let Some(handles) = self.cut_handles.borrow().clone() {
            handles.release(self.scene_pointer(cursor).as_ref(), moved);
        }
        self.mark_dirty();
        true
    }

    /// Gives up a drag of the Cut tool's gizmo (a right press): the plane
    /// goes back where it was. Whether there was one.
    pub(crate) fn cancel_cut_drag(&self) -> bool {
        if self.cut_press.take().is_none() {
            return false;
        }
        if let Some(handles) = self.cut_handles.borrow().clone() {
            handles.release(None, true);
        }
        true
    }

    /// Offers a click in the Transform tab to the Cut tool.
    fn cut_click(&self, cursor: ScreenPoint) -> bool {
        let Some(handles) = self.open_cut() else {
            return false;
        };
        self.scene_pointer(cursor)
            .is_some_and(|pointer| handles.click(&pointer))
    }

    pub(crate) fn set_lasso_handler(&self, handler: Rc<dyn Fn(LassoRegion)>) {
        *self.lasso_handler.borrow_mut() = Some(handler);
    }

    /// Whether the Hollow tab's lasso is armed: it takes the left button.
    fn lasso_armed(&self) -> bool {
        self.session.section() == WorkspaceSection::Hollow
            && self
                .ui
                .upgrade()
                .is_some_and(|ui| ui.get_hollow_lasso_tool())
    }

    /// A left press with the lasso armed starts a stroke there.
    fn lasso_down(&self, cursor: ScreenPoint) -> bool {
        if !self.lasso_armed() {
            return false;
        }
        *self.lasso.borrow_mut() = Some(vec![[cursor.x, cursor.y]]);
        self.show_lasso();
        true
    }

    /// A move while a stroke is drawn: the curve reaches the cursor, a point
    /// every few pixels.
    fn lasso_move(&self, cursor: ScreenPoint) -> bool {
        let mut lasso = self.lasso.borrow_mut();
        let Some(points) = lasso.as_mut() else {
            return false;
        };
        let at = [cursor.x, cursor.y];
        let last = points[points.len() - 1];
        if (at[0] - last[0]).hypot(at[1] - last[1]) >= LASSO_STEP_PX {
            points.push(at);
            drop(lasso);
            self.show_lasso();
        }
        true
    }

    /// The release that closes a stroke: the region behind it, as the
    /// camera sees it now, goes to the Hollow tab. A stroke too small to
    /// enclose anything is dropped.
    fn lasso_up(&self, cursor: ScreenPoint) -> bool {
        let Some(mut points) = self.lasso.borrow_mut().take() else {
            return false;
        };
        points.push([cursor.x, cursor.y]);
        self.show_lasso();
        let region = self.lasso_region(&points);
        let handler = self.lasso_handler.borrow().clone();
        if let (Some(region), Some(handler)) = (region, handler) {
            handler(region);
        }
        true
    }

    /// Drops a lasso stroke in flight: Esc or a right press. Whether there
    /// was one.
    pub(crate) fn cancel_lasso(&self) -> bool {
        if self.lasso.borrow_mut().take().is_none() {
            return false;
        }
        self.show_lasso();
        true
    }

    /// Draws the stroke in flight on the viewport, or nothing.
    fn show_lasso(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let commands = match self.lasso.borrow().as_ref() {
            Some(points) if points.len() > 1 => lasso_commands(points),
            _ => String::new(),
        };
        ui.set_hollow_lasso_commands(commands.into());
    }

    /// The world region behind the closed curve `points` (viewport pixels),
    /// as the camera sees it now: a cone from the eye in perspective, a
    /// prism along the view in orthographic. `None` for a curve that
    /// encloses no area.
    fn lasso_region(&self, points: &[[f32; 2]]) -> Option<LassoRegion> {
        let viewport = self.render.viewport()?;
        let camera_slot = self.camera.camera_cell().borrow();
        let camera = camera_slot.as_ref()?;
        lasso_region(
            camera,
            crate::mesh_render::default_fov_y(),
            [viewport.width(), viewport.height()],
            points,
        )
    }

    /// Drops a stroke in flight: Esc or a right press. Whether there was one.
    pub(crate) fn cancel_brush_stroke(&self) -> bool {
        let Some(brush) = self.surface_brush.borrow().clone() else {
            return false;
        };
        if !brush.stroking() {
            return false;
        }
        brush.end(false);
        self.mark_dirty();
        true
    }

    /// The armed tab tool changed (or the tab did): a preview of the old tool
    /// no longer applies.
    pub(crate) fn on_tab_tools_changed(&self) {
        self.cancel_support_drag();
        self.set_preview_overlay(None);
        // The lasso hides the supports, and they come back with it (#512).
        self.render.request_redraw();
        if let Some(ui) = self.ui.upgrade() {
            crate::breadcrumbs::note(&format!(
                "tools: support tool {}, drain tool {}",
                ui.get_support_tool(),
                ui.get_hollow_drain_tool()
            ));
        }
    }

    /// Clears the dispatcher's cached hover cursor when the cursor leaves the
    /// viewport, so the paste path falls back to the viewport-centre placement.
    pub(crate) fn on_pointer_leave(&self) {
        if let Some(brush) = self.armed_brush() {
            brush.hover(None, false);
            self.mark_dirty();
        }
        self.input.on_pointer_leave();
        if let Some(handles) = self.open_cut() {
            if handles.hover(None) {
                self.mark_dirty();
            }
        } else {
            self.set_preview_overlay(None);
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_hollow_drain_hover(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, false, false);
        }
    }

    /// Latches a right-button press position. The release path classifies the
    /// gesture as click (context menu) or drag (camera orbit) by comparing
    /// displacement against the click threshold.
    pub(crate) fn on_right_pointer_down(&self, x: f32, y: f32) {
        // A right press drops a stroke or a Supports drag in flight, and is
        // no more.
        if self.cancel_lasso()
            || self.cancel_brush_stroke()
            || self.cancel_support_drag()
            || self.cancel_cut_drag()
        {
            self.right_dropped_drag.set(true);
            return;
        }
        self.input.on_right_pointer_down(ScreenPoint { x, y });
    }

    /// Classifies a right-button release. `Some(press_origin)` when the gesture
    /// stayed within the click threshold (caller opens the context menu at that
    /// pixel); `None` for a drag the orbit gesture already consumed or a
    /// release without a matching press.
    pub(crate) fn on_right_pointer_up(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        if self.right_dropped_drag.take() {
            return None;
        }
        self.input
            .on_right_pointer_up(ScreenPoint { x, y })
            .map(|p| (p.x, p.y))
    }

    /// Runs the picker at `(x, y)` without changing the document's selection.
    /// Returns the `NodeId` of the leaf a click there selects (outside the
    /// Supports tab, the model of the supports hit), or `None` on a miss,
    /// with no scene loaded, or with no camera ready.
    pub(crate) fn pick_node_at(&self, x: f32, y: f32) -> Option<sc_scene::NodeId> {
        self.input
            .pick_at(
                self.session.document(),
                self.camera.camera_cell(),
                self.render.viewport(),
                ScreenPoint { x, y },
                self.supports_selectable(),
            )
            .flatten()
    }

    /// Runs the picker at `cursor` through InputDispatch (which builds the ray
    /// and finds the leaf a click selects) and applies the result through the
    /// document's selection setters: a hit selects its leaf, a miss runs the
    /// Esc cascade.
    fn run_pick_at(&self, cursor: ScreenPoint) {
        let Some(pick) = self.input.pick_at(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
            self.supports_selectable(),
        ) else {
            return;
        };
        match pick {
            Some(node_id) => self
                .session
                .document()
                .set_selection(Selection::One(LeafId(node_id))),
            None => self.on_clear_selection(),
        }
    }

    /// Pastes the clipboard's fragment into the current scene near the cursor's
    /// projection on the build plate. Returns whether the paste landed: `false`
    /// when a precondition fails (no scene loaded, no camera or viewport ready,
    /// fragment empty, the cursor ray misses the plane of the plate's floor).
    ///
    /// Cursor anchor: the last cached pointer-move position if the cursor still
    /// hovers the viewport; otherwise the viewport centre. The world ray is
    /// built from that pixel, intersected with the plane of the plate's floor,
    /// and the result feeds the cascade-aware collision-avoidance placement
    /// search.
    pub(crate) fn paste_clipboard_at_cursor(&self, clipboard: &Clipboard) -> bool {
        let Some(viewport) = self.render.viewport() else {
            return false;
        };
        let anchor = self.input.last_hover_cursor().unwrap_or(ScreenPoint {
            x: viewport.width() * 0.5,
            y: viewport.height() * 0.5,
        });
        self.paste_clipboard_with_anchor(clipboard, anchor, viewport)
    }

    /// Pastes anchored on an explicit logical-pixel position, so a context-menu
    /// paste lands at the right-click pixel.
    pub(crate) fn paste_clipboard_at_pixel(&self, clipboard: &Clipboard, x: f32, y: f32) -> bool {
        let Some(viewport) = self.render.viewport() else {
            return false;
        };
        self.paste_clipboard_with_anchor(clipboard, ScreenPoint { x, y }, viewport)
    }

    /// Pastes at `anchor`: builds the ray, projects it onto the plate's floor,
    /// computes the placement against the destination scene's obstacles, and
    /// pushes the resulting compound through the document. Returns `true` only
    /// when the paste landed.
    fn paste_clipboard_with_anchor(
        &self,
        clipboard: &Clipboard,
        anchor: ScreenPoint,
        viewport: super::types::ViewportSize,
    ) -> bool {
        if !self.allows_layout_tools() {
            return false;
        }
        let camera_slot = self.camera.camera_cell().borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return false;
        };
        let ray = picking::ray_from_pixel(
            camera,
            crate::mesh_render::default_fov_y(),
            viewport.width(),
            viewport.height(),
            anchor.x,
            anchor.y,
        );
        let floor_z = self.build_volume().volume().envelope_min_mm()[2];
        let Some((world_x, world_y)) = picking::ray_z_plane_intersection(&ray, floor_z as f32)
        else {
            return false;
        };
        drop(camera_slot);
        match clipboard::paste(
            self.session.document(),
            clipboard,
            [world_x as f64, world_y as f64],
            floor_z,
        ) {
            Ok(Some(_)) => {
                self.mark_dirty();
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.show_edit_rejected(error);
                false
            }
        }
    }

    fn mirror_selection_to_ui(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let document = self.session.document();
            let idx = document
                .selection_primary_node_id()
                .and_then(|id| document.with_scene(|s| s.leaf_index_of(id)).flatten())
                .map_or(-1, |i| i as i32);
            ui.set_selected_object_index(idx);
            let rows = selected_rows(document);
            ui.set_selected_object_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
            ui.set_selected_object_count(document.selection_len() as i32);
        }
    }

    /// Whether a gizmo drag or a modal G/R/S gesture is in flight.
    fn gesture_active(&self) -> bool {
        self.input.modal.get().is_some()
            || matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. })
    }

    /// Pushes the selected object's size, position, rotation and scale to the
    /// Transform inspector, when a scene event asked for it. During a gesture
    /// only the matrix is read (rotation, scale) and the position follows the
    /// translation from the last full readout; the tight world box, a walk
    /// over the vertices, waits for the gesture to end.
    fn refresh_readout(&self) {
        if !self.readout_owed.replace(false) {
            // The gesture aids are in screen space: they follow an orbit
            // during a gesture even when the object did not change.
            if self.gesture_active()
                && let Some(ui) = self.ui.upgrade()
            {
                self.refresh_gesture_overlay(&ui);
            }
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let document = self.session.document();
        let Some(id) = document.selection_primary_node_id() else {
            self.readout_base.set(None);
            ui.set_selection_valid(false);
            return;
        };
        let gesture = self.gesture_active();
        self.refresh_gesture_overlay(&ui);
        let base = self.readout_base.get();
        let translation = |world: Transform3D| match world {
            Transform3D::Identity => [0.0; 3],
            Transform3D::Affine(a) => *a.translation(),
        };
        let computed = document
            .with_scene(|spatial| {
                let query = spatial.leaf(id)?;
                let world = query.world_transform();
                if gesture && let Some((full, t0)) = base {
                    let t = translation(world);
                    return Some(Readout {
                        size: full.size,
                        model_size: model_size(&query, world).unwrap_or(full.model_size),
                        position: [0, 1, 2].map(|k| full.position[k] + t[k] - t0[k]),
                        ..cheap_readout(world)
                    });
                }
                let full = readout(&query)?;
                self.readout_base.set(Some((full, translation(world))));
                Some(full)
            })
            .flatten();
        let Some(r) = computed else {
            ui.set_selection_valid(false);
            return;
        };
        ui.set_selection_valid(true);
        ui.set_selection_size_x(r.size[0] as f32);
        ui.set_selection_size_y(r.size[1] as f32);
        ui.set_selection_size_z(r.size[2] as f32);
        ui.set_selection_model_x(r.model_size[0] as f32);
        ui.set_selection_model_y(r.model_size[1] as f32);
        ui.set_selection_model_z(r.model_size[2] as f32);
        ui.set_selection_tilted(r.tilted);
        ui.set_selection_pos_x(r.position[0] as f32);
        ui.set_selection_pos_y(r.position[1] as f32);
        ui.set_selection_pos_z(r.position[2] as f32);
        ui.set_selection_rot_x(r.rotation_deg[0] as f32);
        ui.set_selection_rot_y(r.rotation_deg[1] as f32);
        ui.set_selection_rot_z(r.rotation_deg[2] as f32);
        ui.set_selection_scale(r.scale_pct as f32);
    }

    /// The viewport aids of a gesture in flight (BS-69): what it has done so
    /// far (distance, degrees, scale) in the chip and next to the dragged
    /// handle (or the cursor, in G/R/S), the line along the axis through the
    /// start position in the axis colour, as Blender draws it, and while
    /// moving a dashed line from the start. Cleared when no gesture runs.
    fn refresh_gesture_overlay(&self, ui: &MainWindow) {
        let document = self.session.document();
        let modal = self.input.modal.get();
        let drag = match self.input.gizmo_fsm().current() {
            GizmoState::Drag {
                op,
                original,
                pivot,
                node_id,
            } => Some((op, original, pivot, node_id)),
            _ => None,
        };
        let (node, local_before, kind, axis, parent_known) = match (modal, drag) {
            (Some(g), _) => {
                let kind = match g.op {
                    ModalOp::Move => GestureKind::Move,
                    ModalOp::Rotate => GestureKind::Rotate,
                    ModalOp::Scale => GestureKind::Scale,
                };
                (g.node_id, g.original, kind, g.axis, Some(g.parent))
            }
            (None, Some((op, original, _, node_id))) => {
                let kind = match op {
                    DragOp::Translate { .. } | DragOp::Plane { .. } => GestureKind::Move,
                    DragOp::Rotate { .. } => GestureKind::Rotate,
                    DragOp::ScaleAxis { .. } | DragOp::ScaleUniform { .. } => GestureKind::Scale,
                };
                let axis = super::gesture_overlay::drag_axis(op).map(Axis::index);
                (node_id, original, kind, axis, None)
            }
            (None, None) => {
                ui.set_gesture_readout(slint::SharedString::default());
                ui.set_gesture_tag(slint::SharedString::default());
                ui.set_axis_guide_visible(false);
                ui.set_axis_guide_2_visible(false);
                ui.set_move_trail_visible(false);
                return;
            }
        };
        let computed = document
            .with_scene(|spatial| {
                let query = spatial.leaf(node)?;
                let parent = parent_known.unwrap_or_else(|| query.parent_world());
                let text = gesture_delta_text(
                    kind,
                    parent.compose(&local_before),
                    query.world_transform(),
                    axis,
                );
                // With several objects selected the gizmo, and so the trail,
                // follows their shared centre (#13).
                let pivot_now =
                    super::gizmo_fsm::gizmo_pivot(spatial, node, &document.selection_node_ids())?;
                let camera_slot = self.camera.camera_cell().borrow();
                let camera = camera_slot.as_ref()?;
                let viewport = self.render.viewport()?;
                let screen = Screen {
                    camera,
                    width: viewport.width(),
                    height: viewport.height(),
                    look: self.input.gizmo_look(),
                };
                let view = match (&modal, drag) {
                    (Some(gesture), _) => GestureView::Modal { gesture, pivot_now },
                    (None, Some((op, _, pivot_start, _))) => GestureView::Drag {
                        op,
                        pivot_start,
                        pivot_now,
                    },
                    (None, None) => return None,
                };
                Some((text, super::gesture_overlay::compose(&view, &screen)))
            })
            .flatten();
        let Some((text, aids)) = computed else {
            ui.set_gesture_readout(slint::SharedString::default());
            ui.set_gesture_tag(slint::SharedString::default());
            ui.set_axis_guide_visible(false);
            ui.set_axis_guide_2_visible(false);
            ui.set_move_trail_visible(false);
            return;
        };
        match aids.tag_anchor {
            Some((x, y)) if !text.is_empty() => {
                ui.set_gesture_tag(super::gesture_overlay::tag_text(&text).into());
                ui.set_gesture_tag_x(x);
                ui.set_gesture_tag_y(y);
            }
            _ => ui.set_gesture_tag(slint::SharedString::default()),
        }
        ui.set_gesture_readout(text.into());
        match aids.guide {
            Some((commands, [r, g, b, a])) => {
                ui.set_axis_guide_commands(commands.into());
                ui.set_axis_guide_color(slint::Color::from_argb_u8(a, r, g, b).into());
                ui.set_axis_guide_visible(true);
            }
            None => ui.set_axis_guide_visible(false),
        }
        match aids.guide2 {
            Some((commands, [r, g, b, a])) => {
                ui.set_axis_guide_2_commands(commands.into());
                ui.set_axis_guide_2_color(slint::Color::from_argb_u8(a, r, g, b).into());
                ui.set_axis_guide_2_visible(true);
            }
            None => ui.set_axis_guide_2_visible(false),
        }
        match aids.trail {
            Some((commands, (x, y))) => {
                ui.set_move_trail_commands(commands.into());
                ui.set_move_trail_start_x(x);
                ui.set_move_trail_start_y(y);
                ui.set_move_trail_visible(true);
            }
            None => ui.set_move_trail_visible(false),
        }
    }

    /// Applies a value typed into the Transform inspector (see
    /// [`NumericEdit::from_ui`] for the codes) as one undoable edit. Only in
    /// the Transform tab, with a selection, and never mid-gesture.
    pub(crate) fn on_numeric_edit(&self, code: i32, value: f32) {
        // The lift (and dropping to the plate) is also a Supports setting:
        // the room under the model the supports fill.
        let lift_in_supports =
            self.session.section() == WorkspaceSection::Supports && matches!(code, 2 | 10);
        if !(self.allows_layout_tools() || lift_in_supports) || self.gesture_active() {
            return;
        }
        let document = self.session.document();
        let Some(id) = document.selection_primary_node_id() else {
            return;
        };
        // Several selected: each drops to the plate on its own (#516).
        if code == 10 && document.selection_len() > 1 {
            self.drop_each_to_plate();
            return;
        }
        if document.selection_len() > 1 && !lift_in_supports {
            return;
        }
        let volume = self.build_volume();
        let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
        let centre = [f64::from(lo.x + hi.x) * 0.5, f64::from(lo.y + hi.y) * 0.5];
        let Some(edit) = NumericEdit::from_ui(code, f64::from(value), centre) else {
            return;
        };
        let change = document
            .with_scene(|spatial| {
                let query = spatial.leaf(id)?;
                let before = query.local_transform();
                let after = super::numeric_transform::apply(&query, edit)?;
                Some((before, after))
            })
            .flatten();
        let Some((before, after)) = change else {
            self.readout_owed.set(true);
            return;
        };
        let applied = document.apply_edit(sc_scene_edit::SceneEdit::SetLocalTransform {
            node_id: id,
            before,
            after,
        });
        match applied {
            Ok(()) => document.push_edit(sc_scene_edit::SceneEdit::SetLocalTransform {
                node_id: id,
                before,
                after,
            }),
            Err(EditNotApplied::Rejected(reason)) => self.show_edit_rejected(reason),
            Err(_) => {}
        }
        self.readout_owed.set(true);
        self.mark_dirty();
    }

    /// Drop to plate with several objects selected: each one's lowest point
    /// to the plate, in one undo step (#516). Supports selected with them
    /// are not dropped on their own: they follow their model.
    fn drop_each_to_plate(&self) {
        let document = self.session.document();
        let edits: Vec<sc_scene_edit::SceneEdit> = document
            .with_scene(|spatial| {
                document
                    .selection_node_ids()
                    .into_iter()
                    .filter(|&id| !shows_supports(spatial, id))
                    .filter_map(|id| {
                        let query = spatial.leaf(id)?;
                        let before = query.local_transform();
                        let after =
                            super::numeric_transform::apply(&query, NumericEdit::DropToPlate)?;
                        (after != before).then_some(sc_scene_edit::SceneEdit::SetLocalTransform {
                            node_id: id,
                            before,
                            after,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !edits.is_empty() {
            let edit = sc_scene_edit::SceneEdit::Compound(edits);
            match document.apply_edit(edit.clone()) {
                Ok(()) => document.push_edit(edit),
                Err(EditNotApplied::Rejected(reason)) => self.show_edit_rejected(reason),
                Err(_) => {}
            }
        }
        self.readout_owed.set(true);
        self.mark_dirty();
    }

    pub(crate) fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
        self.paint_owed.set(true);
        crate::breadcrumbs::note("window: graphics ready");
        // Mint a fresh token for this setup. Both the loader snapshot (consumed
        // by the worker-side `GpuScene::build`) and the renderer (built below)
        // carry it. The 2 values match for the entire lifetime of this setup;
        // the next teardown -> setup cycle mints a different token, which is
        // how `Renderer::install_scene` detects an out-of-date worker-built
        // scene.
        let device_token = crate::mesh_render::DeviceToken::next();
        let device_loss = self.session.loader().device_loss();
        if device_loss.is_lost() {
            self.session.loader().note_device_lost();
            return;
        }
        // The renderer is built before the device is published, so a refused
        // renderer leaves the window with no device to load into.
        match self.render.handle_rendering_setup(
            device.clone(),
            device_token,
            queue.clone(),
            device_loss,
        ) {
            Ok(()) => {
                self.session.loader().set_gpu(device, device_token, queue);
                self.post_renderer_install();
            }
            Err(err) => self.post_renderer_refused(err),
        }
    }

    /// Handles a `RenderingSetup` that the viewport cannot use: the window is
    /// rendering through a graphics API other than the wgpu one this build
    /// draws with, so no device will be published for it and the viewport stays
    /// empty. Records which API arrived so anything that needs a device can
    /// explain the absence.
    pub(crate) fn handle_unsupported_graphics_api(&self, api: String) {
        self.session.loader().note_unsupported_graphics_api(api);
    }

    /// Records the refusal as the window's reason for having no device, shows
    /// it once, and lets a load parked for this setup report itself against
    /// that reason.
    pub(super) fn post_renderer_refused(&self, err: RendererInitError) {
        self.session
            .loader()
            .note_renderer_refused(StageRefusal::new(err.stage, &err.source));
        self.session
            .dialogs()
            .show_error_deferred(move || ErrorTexts {
                heading: ctr!("ErrorDialog", "The 3D view could not start"),
                context: Some(ctr!(
                    "ErrorDialog",
                    "Files cannot be opened in this window."
                )),
                message: err.to_string().into(),
                details: err.source.to_string().into(),
            });
        if let Some(path) = self.session.pending_load().take() {
            self.start_pending_load(path);
        }
    }

    /// Rearms the loader's failure gates, flags an upload when a scene is
    /// loaded, requests a redraw, and starts any load that was queued while no
    /// device was available.
    pub(super) fn post_renderer_install(&self) {
        self.session.loader().reset_upload_failures();
        self.session.loader().reset_render_failures();
        if self.session.document().is_loaded() {
            self.render.update_frame(FrameState::with_upload_owed);
        }
        // Always request a redraw after RenderingSetup: even with no scene, the
        // plate is part of the steady-state output.
        self.request_redraw();
        // The loader now holds the device, so start any path queued before this
        // point (for example a path given on the command line). `begin_load`
        // returns `None` when a load is already in flight on this fresh setup,
        // which is rare but possible when a teardown -> setup cycle races a
        // click on Open; the queued path is then dropped, as
        // `AppState::dispatch_path_to_loader` drops a path for a busy window.
        if let Some(path) = self.session.pending_load().take() {
            self.start_pending_load(path);
        }
    }

    /// Releases GPU-owning state before the graphics context goes away. The
    /// scene (CPU-side) and the camera stay in the controller, so the next
    /// `RenderingSetup` re-uploads without losing the user's view; that setup
    /// flags the upload itself when a scene is present. The loader's GPU slot
    /// is cleared too, so a load started between teardown and the next setup
    /// finds no device and reports why, and no worker builds buffers against an
    /// unowned device.
    pub(crate) fn handle_rendering_teardown(&self) {
        self.session.loader().clear_gpu();
        self.render.handle_rendering_teardown();
    }

    /// Steps the keyboard-focus integrator (which can mutate the scene
    /// transform), drains any pending GPU upload, and initialises the orbit
    /// camera when none exists. Runs before the paint work in
    /// [`Self::handle_before_rendering`] so the paint reads a consistent view
    /// of the scene.
    ///
    /// Returns `false` when paint should be skipped: the renderer is absent or
    /// the device is lost. A failed upload does not skip the paint: it drops
    /// the renderer's scene and lets the frame fall back to the plate.
    fn on_scene_change(&self) -> bool {
        // Step the keyboard-focus integrator first: it mutates the scene's
        // transform and marks dirty, and the paint pass below must read that
        // change in this same frame.
        self.input.integrate_keyboard_arrows(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
        );

        let mut renderer_slot = self.render.renderer_cell().borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return false;
        };

        if self.session.loader().device_loss().is_lost() {
            // No dialog: a lost device is reported once per process.
            self.render.update_frame(FrameState::upload_cleared);
            self.render.update_frame(FrameState::after_render);
            return false;
        }

        if self.render.frame().upload_owed() {
            crate::breadcrumbs::note("gpu: scene upload");
            let upload = self
                .session
                .document()
                .with_scene(|spatial| renderer.upload_scene(spatial.graph()));
            crate::breadcrumbs::note(&format!(
                "gpu: scene upload ok={}",
                matches!(upload, Some(Ok(())))
            ));
            match upload {
                Some(Ok(())) => {
                    self.render.update_frame(FrameState::after_upload_success);
                    self.session.loader().note_upload_success();
                }
                Some(Err(err)) => {
                    // The upload failed, so whatever scene the renderer still
                    // holds no longer mirrors the live graph. Drop it and carry
                    // on with the frame: the paint falls back to the bare
                    // plate, which keeps the camera and the rest of the window
                    // responsive.
                    renderer.clear_scene();
                    // The flag is cleared: a standing flag would re-run the
                    // failing build and this scene drop on every later dirty
                    // event, churning the render-target allocations for as long
                    // as the failure lasts. Nothing is lost, because every path
                    // that changes what needs uploading sets the flag itself:
                    // `post_renderer_install`, the scene-install fallback, and
                    // each structural edit.
                    self.render.update_frame(FrameState::upload_cleared);
                    self.session.loader().note_upload_failure(err);
                }
                None => {
                    // The flag is set, but the scene was cleared before this
                    // frame.
                    self.render.update_frame(FrameState::upload_cleared);
                }
            }
        }

        // Initialise the orbit camera when the slot is empty. With the plate
        // visible at first paint, framing comes from the union of the scene's
        // world AABB (when uploaded) and the build envelope. Skipped if a
        // camera already exists (re-upload across a RenderingTeardown -> Setup
        // cycle preserves the user's view).
        if !self.camera.is_initialised() {
            let scene_bounds = renderer.scene_bounds();
            self.camera
                .replace_with_framing(scene_bounds, self.session.build_volume());
        }

        true
    }

    /// Reconciles the slice-view panel image with the current cap key.
    /// `cap_key` is `Some` exactly when the 3D cut is active (Slice section,
    /// held slice, a layer with a top edge). When it differs from the last
    /// pushed key, rebuilds the cross-section image, or pushes an empty one
    /// when the key is `None`; an unchanged key skips the rebuild, so repeated
    /// frames do not re-upload the image. The image is kept current while the
    /// Slice section shows a held slice, so the View-menu toggle shows it at
    /// once without a refresh of its own.
    pub(crate) fn reconcile_slice_view_image(
        &self,
        ui: &MainWindow,
        cap_key: Option<u64>,
        layer: u32,
        build_volume: &BuildVolume,
    ) {
        if self.slice_view_key.get() == cap_key {
            return;
        }
        let image = match cap_key {
            Some(_) => self
                .cap_raster(layer, build_volume)
                .map(|raster| {
                    slint::Image::from_rgba8(crate::slice_cap::cap_raster_to_rgba(&raster))
                })
                .unwrap_or_default(),
            None => slint::Image::default(),
        };
        ui.set_slice_view_image(image);
        self.slice_view_key.set(cap_key);
    }

    /// Paints the current frame: snapshots the viewport size, builds the gizmo
    /// frame against the live scene, renders, and hands the resulting texture
    /// to Slint via `set_mesh_rendering`. Bails early when no paint is owed,
    /// when any of the required pieces are missing (renderer, UI handle,
    /// viewport, or camera), or when the viewport's width or height rounds to 0
    /// pixels.
    fn on_paint(&self) {
        if !self.render.frame().redraw_owed() || self.render.hold_frame() {
            return;
        }

        let mut renderer_slot = self.render.renderer_cell().borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };
        let marks_shown = self.session.section() == WorkspaceSection::Transform;
        let overlay = self
            .repair_marks
            .over(self.overlay.borrow().as_ref(), marks_shown);
        renderer.set_overlay(overlay.as_ref());
        let hints = (self.session.section() == WorkspaceSection::Supports)
            .then(|| self.hint_marks.borrow().clone())
            .flatten();
        renderer.set_hint_marks(hints.as_ref());
        let overhang_map = self.overhang_map();
        renderer.set_overhang_map(overhang_map);
        if overhang_map.is_some() {
            renderer.set_support_tips(&self.support_tips());
        }
        renderer.set_raft_top(self.raft_top_under_plate());
        let (shown_up_to, lines) = self.under_plate_supports();
        renderer.set_support_lines(shown_up_to, lines.as_ref());
        renderer.set_supports_hidden(self.supports_hidden());
        renderer.set_focus(&self.focus());

        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.paint_owed.replace(false) {
            let layers = self.paint_layers();
            renderer.set_surface_paint(&layers, surface_paint_tint(&ui));
        }
        let ring = self
            .armed_brush()
            .filter(|_| self.paint_shown.get())
            .and_then(|brush| brush.ring());
        renderer.set_brush_ring(ring);

        let Some(vp) = self.render.viewport() else {
            return;
        };
        let h_logical = vp.height();
        let scale = ui.window().scale_factor();
        let width = (vp.width() * scale).round() as u32;
        let height = (h_logical * scale).round() as u32;
        if width == 0 || height == 0 {
            return;
        }

        let (eff_w, eff_h) = self.render.bucketed_size(width, height);

        let camera_slot = self.camera.camera_cell().borrow();
        let Some(camera) = camera_slot.as_ref() else {
            return;
        };

        let build_volume = self.session.build_volume();

        // Slice-view clip height. The slider's layer top clips the mesh, but
        // only in the Slice section with a held slice. The slider value is read
        // here on the paint thread; it holds whole layer numbers as a float, so
        // round it to the layer index.
        let selected_layer = ui.get_selected_layer_index().round().max(0.0) as u32;
        let clip_z = crate::workspace_section::clip_z_from(
            self.session.section(),
            self.session.document().slice_layer_top_z(selected_layer),
        );

        // Slice-view cap: fill the cut with the selected layer's coverage so it
        // reads solid. Keyed on the slice generation plus the layer, so a
        // camera orbit reuses the texture and a re-slice rebuilds it; `None`
        // (nothing clipped) clears the cap.
        // The highlighted finding (#409) rides in the key: a click rebuilds
        // the selected layer's cap, and only that.
        let cap_key = clip_z.map(|_| {
            let mut key = DefaultHasher::new();
            (
                self.session.document().slice_generation(),
                selected_layer,
                self.highlight_generation.get(),
            )
                .hash(&mut key);
            key.finish()
        });
        let mut build_cap = || self.cap_raster(selected_layer, &build_volume);
        match renderer.update_cap(cap_key, &mut build_cap) {
            Ok(()) | Err(RenderError::DeviceLost) => {}
            Err(err) => self.session.loader().note_render_failure(&err),
        }

        // The word on the plate's front tab and the printer's name under it,
        // keyed on the text so a language or printer change rebuilds it and an
        // orbit does not.
        // With the plate's size beside it, as a drawing's title block reads
        // (#507).
        let front_word = format!(
            "{}  ·  {} × {} mm",
            ctr!("MeshViewer", "FRONT"),
            plate_length(build_volume.width_mm()),
            plate_length(build_volume.depth_mm())
        );
        let printer_name = self.plate_printer_name.borrow().clone();
        let mut hasher = DefaultHasher::new();
        front_word.as_str().hash(&mut hasher);
        printer_name.hash(&mut hasher);
        let mut build_label =
            || crate::front_label::rasterise_label(front_word.as_str(), printer_name.as_deref());
        match renderer.update_front_label(hasher.finish(), &mut build_label) {
            Ok(()) | Err(RenderError::DeviceLost) => {}
            Err(err) => self.session.loader().note_render_failure(&err),
        }

        // The numbers along the plate's rulers, laid out for this plate.
        let corner = build_volume.envelope_min_mm();
        let (plate_x, plate_y) = (build_volume.width_mm(), build_volume.depth_mm());
        let mut marks_key = DefaultHasher::new();
        [corner.x, corner.y, plate_x, plate_y]
            .map(f32::to_bits)
            .hash(&mut marks_key);
        let mut build_marks =
            || crate::front_label::rasterise_plate_marks([corner.x, corner.y], plate_x, plate_y);
        match renderer.update_plate_marks(marks_key.finish(), &mut build_marks) {
            Ok(()) | Err(RenderError::DeviceLost) => {}
            Err(err) => self.session.loader().note_render_failure(&err),
        }

        // Slice-view panel: mirror the same cut into the 2D panel image, keyed
        // on the same cap key so it rebuilds only when the layer or the slice
        // generation changes. Because this runs in the paint pass, every
        // trigger that repaints (slider, section, scene, slice completion)
        // refreshes the panel with no extra wiring.
        self.reconcile_slice_view_image(&ui, cap_key, selected_layer, &build_volume);

        if self.render.is_refused_size((eff_w, eff_h)) {
            self.render.update_frame(FrameState::after_render);
            return;
        }

        // Render path. With a scene installed, the closure inside `with_scene`
        // composes the gizmo frame against the live scene and renders the union
        // (plate and objects). With no scene, `with_scene` returns `None` and
        // the fallback below renders the plate alone with neither selection nor
        // gizmo.
        let texture = self
            .session
            .document()
            .with_scene(|spatial_for_render| {
                let selection_node_id = self.session.document().selection_primary_node_id();
                let group = self.session.document().selection_node_ids();
                let mut highlighted = group.clone();
                highlighted.sort_unstable();
                // The selection highlight stays visible in every section
                // (selection is live everywhere). The gizmo is a transform
                // tool, so its handles compose in Prepare and Transform:
                // only the selection handed to the composer is gated, and the
                // object tint keeps the full selection.
                let gizmo_selection =
                    if self.allows_transform_gizmo() && !self.brush_on() && !self.cut_on() {
                        selection_node_id
                    } else {
                        None
                    };
                let gizmo_frame = match self.open_cut() {
                    // The Cut tool's gizmo in the object's gizmo's place (#56).
                    Some(handles) => self.cut_gizmo_frame(&*handles, camera, h_logical),
                    None => GizmoFrameComposer::compose(
                        &self.input,
                        spatial_for_render,
                        camera,
                        gizmo_selection,
                        &group,
                        h_logical,
                    ),
                };

                renderer.render(
                    Some(SceneRender {
                        graph: spatial_for_render.graph(),
                        selection: &highlighted,
                        gizmo: gizmo_frame,
                        clip_z,
                    }),
                    build_volume,
                    camera,
                    eff_w,
                    eff_h,
                )
            })
            .unwrap_or_else(|| renderer.render(None, build_volume, camera, eff_w, eff_h));

        let texture = match texture {
            Ok(Some(texture)) => texture,
            Ok(None) => return,
            Err(RenderError::DeviceLost) => {
                self.render.update_frame(FrameState::after_render);
                return;
            }
            Err(err) => {
                self.render.note_render_refused((eff_w, eff_h));
                self.session.loader().note_render_failure(&err);
                return;
            }
        };
        self.render.note_render_ok();
        self.session.loader().note_render_success();
        self.render.update_frame(FrameState::after_render);
        if renderer.overhang_map_pending() {
            self.arm_map_wait();
        }
        match slint::Image::try_from(texture) {
            Ok(image) => ui.set_mesh_rendering(image),
            Err(err) => self
                .session
                .dialogs()
                .show_error_deferred(move || ErrorTexts {
                    heading: ctr!("ErrorDialog", "Failed to display the 3D model"),
                    context: Some(ctr!(
                        "ErrorDialog",
                        "The rendered texture could not be handed to the UI."
                    )),
                    message: err.to_string().into(),
                    details: format!("{err:?}").into(),
                }),
        }
    }

    /// The paint to show now: every model's detail, while it shows.
    fn paint_layers(&self) -> Vec<crate::mesh_render::PaintLayer> {
        if !self.paint_shown.get() {
            return Vec::new();
        }
        self.session
            .document()
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .surface_tags_iter()
                    .filter(|(_, kind, _)| *kind == sc_scene::PaintKind::Detail)
                    .map(|(leaf, _, tags)| crate::mesh_render::PaintLayer {
                        leaf,
                        tags: tags.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn handle_before_rendering(&self) {
        if self.probe.borrow().is_some() {
            return;
        }
        self.settle_map_wait();
        self.refresh_readout();
        if !self.on_scene_change() {
            return;
        }
        self.on_paint();
    }
}

/// The colour of painted faces, from the theme; its alpha is how much it
/// covers the model.
fn surface_paint_tint(ui: &MainWindow) -> [f32; 4] {
    let color = ui.global::<crate::Theme>().get_surface_paint();
    [
        f32::from(color.red()) / 255.0,
        f32::from(color.green()) / 255.0,
        f32::from(color.blue()) / 255.0,
        f32::from(color.alpha()) / 255.0,
    ]
}

/// The engine's self-support angle before a window's options reach it.
fn default_tip_reach_mm() -> f64 {
    sc_geom::df_supports::DfSupportOptions::default().tip_reach_mm()
}

fn default_overhang_angle_deg() -> f32 {
    sc_geom::df_supports::DfSupportOptions::default().self_support_angle_deg() as f32
}

/// Which rows of the object list are selected, in row order: what the list
/// highlights, so it shows what the viewport does (#13).
pub(super) fn selected_rows(document: &SceneDocument) -> Vec<bool> {
    document
        .with_scene(|s| {
            (0..)
                .map_while(|i| s.leaf_id_at_index(i))
                .map(|id| document.is_selected(id))
                .collect()
        })
        .unwrap_or_default()
}

fn axis_from_ui(view: crate::AxisView) -> AxisView {
    match view {
        crate::AxisView::Front => AxisView::Front,
        crate::AxisView::Back => AxisView::Back,
        crate::AxisView::Right => AxisView::Right,
        crate::AxisView::Left => AxisView::Left,
        crate::AxisView::Top => AxisView::Top,
        crate::AxisView::Bottom => AxisView::Bottom,
    }
}

/// How far apart, in pixels, the lasso keeps the points of a stroke.
const LASSO_STEP_PX: f32 = 3.0;
/// The most points a lasso keeps: a longer stroke keeps every few.
const LASSO_MAX_POINTS: usize = 512;
/// The least area, in square pixels, a lasso must enclose to count.
const LASSO_MIN_AREA_PX: f32 = 64.0;

/// The path commands that draw the lasso stroke `points`, closed.
fn lasso_commands(points: &[[f32; 2]]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(points.len() * 16);
    for (i, p) in points.iter().enumerate() {
        let _ = write!(
            out,
            "{} {:.1} {:.1} ",
            if i == 0 { "M" } else { "L" },
            p[0],
            p[1]
        );
    }
    out.push('Z');
    out
}

/// The world region behind the closed curve `points`, in pixels of a
/// viewport of `size`, as `camera` sees it with a vertical field of view of
/// `fov_y`: what the lasso keeps solid (#179). In perspective a cone from the
/// eye, its polygon where each pixel's ray crosses the plane one unit in
/// front of the eye; in orthographic a prism along the view, its polygon
/// where each pixel's ray starts on the camera plane. `None` for a curve that
/// encloses less than [`LASSO_MIN_AREA_PX`].
pub(crate) fn lasso_region(
    camera: &crate::camera::OrbitCamera,
    fov_y: f32,
    size: [f32; 2],
    points: &[[f32; 2]],
) -> Option<LassoRegion> {
    let area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f32>()
        .abs()
        * 0.5;
    if points.len() < 3 || area < LASSO_MIN_AREA_PX {
        return None;
    }
    let step = points.len().div_ceil(LASSO_MAX_POINTS);
    let eye = camera.eye();
    let forward = (camera.target - eye).normalize_or_zero();
    let (right, up) = camera.right_and_up();
    let polygon: Vec<[f32; 2]> = points
        .iter()
        .step_by(step)
        .map(|p| {
            let ray = picking::ray_from_pixel(camera, fov_y, size[0], size[1], p[0], p[1]);
            if camera.orthographic {
                let d = ray.origin - eye;
                [d.dot(right), d.dot(up)]
            } else {
                let depth = ray.direction.dot(forward).max(1e-6);
                [
                    ray.direction.dot(right) / depth,
                    ray.direction.dot(up) / depth,
                ]
            }
        })
        .collect();
    Some(LassoRegion {
        perspective: !camera.orthographic,
        origin: eye.into(),
        u: right.into(),
        v: up.into(),
        w: forward.into(),
        polygon,
    })
}

#[cfg(test)]
mod lasso_region_tests {
    use super::*;
    use crate::camera::OrbitCamera;
    use glam::Vec3;

    /// #512: the supports leave the view while the lasso is armed in the
    /// Hollow tab, and come back with it put away or in another tab.
    #[test]
    fn the_armed_lasso_hides_the_supports() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let window = MainWindow::new().unwrap();
            let ctrl = ViewportController::new_for_test_with_ui(&window);
            ctrl.set_section(WorkspaceSection::Hollow);
            assert!(!ctrl.supports_hidden());
            window.set_hollow_lasso_tool(true);
            assert!(ctrl.supports_hidden());
            ctrl.set_section(WorkspaceSection::Supports);
            assert!(!ctrl.supports_hidden());
            ctrl.set_section(WorkspaceSection::Hollow);
            window.set_hollow_lasso_tool(false);
            assert!(!ctrl.supports_hidden());
        });
    }

    /// #179: the region a lasso makes holds, at any depth, what the camera
    /// shows inside the curve, and nothing outside it: in perspective and in
    /// orthographic, from a slanted view.
    #[test]
    fn the_lasso_region_is_what_the_camera_shows_inside_the_curve() {
        let size = [800.0, 600.0];
        let fov = crate::mesh_render::default_fov_y();
        // A square of pixels a little left of the middle.
        let square = [
            [300.0, 250.0],
            [420.0, 250.0],
            [420.0, 370.0],
            [300.0, 370.0],
        ];
        for orthographic in [false, true] {
            let mut camera = OrbitCamera::new(Vec3::new(10.0, -5.0, 20.0), 60.0);
            camera.yaw = 0.7;
            camera.pitch = 0.5;
            camera.orthographic = orthographic;
            let region = lasso_region(&camera, fov, size, &square)
                .expect("a region")
                .prepared()
                .expect("prepared");
            let at = |px: [f32; 2], depth: f32| {
                let ray = picking::ray_from_pixel(&camera, fov, size[0], size[1], px[0], px[1]);
                let p = ray.origin + ray.direction * depth;
                sc_geom::df_hollow::hollow_df::Vec3::new(p.x, p.y, p.z)
            };
            for depth in [20.0, 60.0, 140.0] {
                assert!(region.contains(at([360.0, 310.0], depth)));
                assert!(region.contains(at([302.0, 252.0], depth)));
                assert!(!region.contains(at([298.0, 310.0], depth)));
                assert!(!region.contains(at([360.0, 380.0], depth)));
                // The curve itself is the region's surface.
                assert!(region.signed_distance_mm(at([420.0, 300.0], depth)).abs() < 1e-2);
            }
        }
    }

    /// A stroke that encloses next to nothing makes no region.
    #[test]
    fn a_lasso_without_area_makes_no_region() {
        let camera = OrbitCamera::new(Vec3::ZERO, 50.0);
        let fov = crate::mesh_render::default_fov_y();
        let line = [[100.0, 100.0], [200.0, 100.0], [300.0, 101.0]];
        assert!(lasso_region(&camera, fov, [800.0, 600.0], &line).is_none());
    }
}

#[cfg(test)]
mod selection_tests {
    use std::sync::Arc;

    use super::*;
    use crate::test_support::scenes::empty_leaf_mesh;
    use sc_scene::{LeafPayload, Scene};

    /// Returns `PointerModifiers::default()`, with all flags off. The selection
    /// tests do not exercise modifier-driven branches, so every press uses
    /// this.
    fn no_mods() -> PointerModifiers {
        PointerModifiers::default()
    }

    /// Builds a graph of `n` objects in the shape a loader produces: all `n`
    /// leaves under a single identity-Group root.
    fn graph_with_n_objects(n: usize) -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for _ in 0..n {
            let leaf = g
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(empty_leaf_mesh())),
                )
                .expect("add leaf");
            g.add_child(root, leaf).expect("attach leaf");
        }
        g
    }

    #[test]
    fn the_projection_survives_opening_and_closing_a_scene() {
        let ctrl = ViewportController::new_for_test();
        ctrl.frame_camera_for_test();
        assert!(ctrl.on_toggle_projection());
        for event in [SceneEvent::Installed, SceneEvent::Cleared] {
            ctrl.handle_scene_event(event);
            ctrl.frame_camera_for_test();
            assert!(ctrl.orthographic_view(), "{event:?}");
            let camera = ctrl.camera.camera_cell().borrow().clone().unwrap();
            assert!(camera.orthographic, "{event:?}");
        }
        assert!(!ctrl.on_toggle_projection());
        ctrl.handle_scene_event(SceneEvent::Installed);
        ctrl.frame_camera_for_test();
        assert!(!ctrl.orthographic_view());
    }

    #[test]
    fn fresh_controller_has_no_selection() {
        let ctrl = ViewportController::new_for_test();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn select_by_index_records_in_bounds_indices() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_select_by_index(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), Some(2));
    }

    #[test]
    fn select_by_index_clamps_out_of_bounds_to_none() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(2));
        // Pre-seed a valid selection so we can observe the reset.
        ctrl.on_select_by_index(0);
        ctrl.on_select_by_index(99);
        assert_eq!(
            ctrl.selection(),
            None,
            "an out-of-bounds index must collapse to None, not silently keep the prior selection"
        );
    }

    #[test]
    fn select_by_index_with_no_scene_is_a_no_op() {
        let ctrl = ViewportController::new_for_test();
        // No `install_graph_for_test`; the scene cell holds None.
        ctrl.on_select_by_index(0);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn clear_selection_resets_to_none() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(2));
        ctrl.on_select_by_index(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_clears_selection() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), Some(2));
        ctrl.on_close_scene();
        assert_eq!(ctrl.selection(), None);
        // The scene cell is cleared too: a follow-up select must not resurrect
        // a prior index against a missing scene.
        ctrl.on_select_by_index(2);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn select_then_select_same_index_is_idempotent() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(2));
        ctrl.on_select_by_index(0);
        ctrl.clear_frame_state_for_test();
        ctrl.on_select_by_index(0);
        assert!(
            !ctrl.redraw_owed_for_test(),
            "re-selecting the current index should not re-mark the controller dirty"
        );
    }

    #[test]
    fn viewport_click_with_no_camera_is_a_no_op() {
        // The picker requires a camera. A click while the camera slot is empty
        // (before a paint or a finished load frames it) must not panic or
        // change the selection.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(1));
        // No camera installed.
        ctrl.on_pointer_down(ScreenPoint::new(50.0, 50.0), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(50.0, 50.0), no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn viewport_click_with_zero_viewport_is_a_no_op() {
        // With no viewport size stored (`ViewportSize::new` rejects a 0 x 0
        // size), there is no projection to build a ray from, so the controller
        // bails before it divides by the viewport size.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(1));
        // The viewport slot starts as `None`.
        ctrl.on_pointer_down(ScreenPoint::new(0.0, 0.0), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(0.0, 0.0), no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn pointer_up_without_matching_down_is_ignored() {
        // If Slint sends an up without a down (a press captured elsewhere, or
        // input state out of sync), the controller must not run the picker
        // against an unset press.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(1));
        ctrl.on_pointer_up(ScreenPoint::new(50.0, 50.0), no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn cycle_with_no_selection_picks_first_object() {
        // Tab from "nothing selected" doubles as "select the first thing". Both
        // directions land on index 0: the user expects "advance to the first
        // item that exists".
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));

        ctrl.on_clear_selection();
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_forward_advances_through_objects_and_wraps() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_select_by_index(0);
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(2));
        ctrl.on_selection_cycle(1);
        // Wraps from the end back to the start.
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_backward_advances_in_reverse_and_wraps() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_select_by_index(0);
        // From index 0, Shift+Tab wraps to the last object.
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(2));
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_on_empty_scene_is_a_no_op() {
        let ctrl = ViewportController::new_for_test();
        // No scene installed.
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), None);

        // Empty scene installed (loader-shape: Group root with no leaves).
        ctrl.install_graph_for_test(graph_with_n_objects(0));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn cycle_with_single_object_stays_on_it() {
        // 1-object scene -> cycling either direction lands on index 0 every
        // time: mod-N arithmetic with N = 1 gives 0.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(1));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));
        ctrl.on_selection_cycle(1);
        assert_eq!(ctrl.selection(), Some(0));
        ctrl.on_selection_cycle(-1);
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn cycle_clamps_non_unit_deltas() {
        // A delta other than +1 or -1, from a stale Slint binding, still
        // advances by exactly 1 step (or none for delta == 0).
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(graph_with_n_objects(3));
        ctrl.on_select_by_index(0);
        ctrl.on_selection_cycle(99);
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(0);
        // delta == 0 -> step is 0; selection is unchanged.
        assert_eq!(ctrl.selection(), Some(1));
        ctrl.on_selection_cycle(-99);
        assert_eq!(ctrl.selection(), Some(0));
    }
}

#[cfg(test)]
mod viewport_click_tests {
    use super::*;
    use crate::camera::OrbitCamera;
    use crate::gizmo;
    use crate::gizmo::axis::Axis;
    use crate::mesh_render::default_fov_y;
    use crate::picking;
    use crate::scene_document::SceneSubscription;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::{LeafPayload, NodeId, Scene};
    use std::cell::Cell;
    use std::sync::Arc;

    /// Returns `PointerModifiers::default()`, with all 4 flags off.
    fn no_mods() -> PointerModifiers {
        PointerModifiers::default()
    }

    /// Returns `PointerModifiers` with Shift held and the other flags off.
    fn shift_held() -> PointerModifiers {
        PointerModifiers {
            shift: true,
            control: false,
            alt: false,
            meta: false,
        }
    }

    /// Builds a graph with a single identity-Group root, the shape a loader
    /// produces.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    /// Adds a triangle, counter-clockwise seen from +Z, at `translation`, as a
    /// leaf under `parent` (the loader-shape Group root).
    fn add_front_facing_triangle(g: &mut Scene, parent: NodeId, translation: [f32; 3]) {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([-0.5, -0.5, 0.0]),
                Vertex::new([0.5, -0.5, 0.0]),
                Vertex::new([0.0, 0.5, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        };
        let transform = Transform3D::Affine(AffineTransform3D::from_translation([
            translation[0] as f64,
            translation[1] as f64,
            translation[2] as f64,
        ]));
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add leaf");
        g.add_child(parent, leaf).expect("attach leaf");
    }

    /// Returns a camera that looks along -Z at `target` from +Z, so the centre
    /// pixel's ray is the vertical line through `target`, which exercises the
    /// ray pipeline end to end.
    fn camera_looking_down_minus_z_at(target: Vec3, distance: f32) -> OrbitCamera {
        let mut cam = OrbitCamera::new(target, distance);
        cam.set_axis_view(AxisView::Top);
        cam
    }

    #[test]
    fn click_centre_picks_the_triangle_directly_under_the_cursor() {
        let ctrl = ViewportController::new_for_test();
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        add_front_facing_triangle(&mut g, root, [10.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);

        // Camera framed on the first triangle, looking down -Z. Centre pixel of
        // the viewport casts a ray straight at it.
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.set_viewport_for_test(200.0, 100.0);

        ctrl.on_pointer_down(ScreenPoint::new(100.0, 50.0), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(100.0, 50.0), no_mods());
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn click_in_empty_space_clears_the_selection() {
        let ctrl = ViewportController::new_for_test();
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        // A 200x100 viewport with the default gizmo size leaves no empty pixel:
        // HANDLE_PIXEL_LENGTH is 120 px, so the rotate-cone bounding volumes
        // (perspective-projected close to the camera in Top view) span most of
        // the screen. Use a larger viewport so the gizmo covers about 15% of it
        // and the corner is empty.
        ctrl.set_viewport_for_test(800.0, 600.0);

        // Pre-seed a selection so we can observe the clear.
        ctrl.on_select_by_index(0);
        assert_eq!(ctrl.selection(), Some(0));

        // Click at the corner, where there is no triangle.
        ctrl.on_pointer_down(ScreenPoint::new(0.0, 0.0), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(0.0, 0.0), no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn drag_does_not_run_the_picker() {
        // A press -> release that moves farther than the click threshold is a
        // drag, which must not reach the picker: a large pointer move would
        // change the selection.
        let ctrl = ViewportController::new_for_test();
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.set_viewport_for_test(200.0, 100.0);

        // Press over the triangle, release 50 px away -> drag.
        ctrl.on_pointer_down(ScreenPoint::new(100.0, 50.0), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(150.0, 50.0), no_mods());
        assert_eq!(ctrl.selection(), None);
    }

    /// Returns a camera that looks along +Y from `(0, -3, 0)` toward the origin
    /// with up = +Z. The world axes project as: +X = screen-right, +Z =
    /// screen-up, +Y = into the screen. With the gizmo pivot at the origin, the
    /// X handle lies along the centre horizontal of the viewport, so clicks at
    /// `(centre_x + d, centre_y)` land on it.
    fn front_camera() -> OrbitCamera {
        let mut c = OrbitCamera::new(Vec3::ZERO, 1.0);
        c.set_axis_view(AxisView::Front);
        c
    }

    /// Width and height of the viewport used by the gizmo-drag tests. 400x200
    /// is wide enough that the X-axis cone tip (`gizmo_scale * 1.18`, about
    /// 1.89 world units along +X with the front camera at distance 3) lands
    /// well inside the viewport. Smaller viewports clip the cone off-screen and
    /// make cone-tip presses impossible.
    const GIZMO_TEST_VIEWPORT: (f32, f32) = (400.0, 200.0);

    fn install_single_object_at_origin(ctrl: &ViewportController) {
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() = Some(front_camera());
        let (vw, vh) = GIZMO_TEST_VIEWPORT;
        ctrl.set_viewport_for_test(vw, vh);
        ctrl.on_select_by_index(0);
    }

    /// Logical-pixel position of a click on the X-axis cone tip. Computed for
    /// the gizmo test setup (front camera, 400x200 viewport, default FOV, scene
    /// radius 1.0): the X translate hit region spans screen-x [320, 350] at
    /// screen-y about 100, and `(335, 100)` lands inside it.
    const X_CONE_PRESS: (f32, f32) = (335.0, 100.0);

    fn read_translation(ctrl: &ViewportController) -> Vec3 {
        ctrl.session
            .document()
            .with_scene(|spatial| {
                let id = spatial
                    .leaf_id_at_index(0)
                    .expect("test fixture has a leaf");
                match spatial.leaf(id).unwrap().local_transform() {
                    Transform3D::Identity => Vec3::ZERO,
                    Transform3D::Affine(a) => Vec3::new(
                        a.translation()[0] as f32,
                        a.translation()[1] as f32,
                        a.translation()[2] as f32,
                    ),
                }
            })
            .unwrap()
    }

    fn read_linear(ctrl: &ViewportController) -> [[f64; 3]; 3] {
        ctrl.session
            .document()
            .with_scene(|spatial| {
                let id = spatial
                    .leaf_id_at_index(0)
                    .expect("test fixture has a leaf");
                match spatial.leaf(id).unwrap().local_transform() {
                    Transform3D::Identity => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    Transform3D::Affine(a) => *a.linear(),
                }
            })
            .unwrap()
    }

    /// Projects the X-axis midpoint cube's centre to logical-pixel coordinates
    /// for the standard test setup. The cube sits at `pivot + axis_world *
    /// (scale * MIDPOINT_FRACTION)`; with the front camera, that lands on
    /// screen-y = viewport-centre.
    /// Logical-pixel position of the gizmo pivot (the centre ball).
    fn pivot_press_screen(ctrl: &ViewportController) -> (f32, f32) {
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
        let (vw, vh) = ctrl.viewport_for_test().unwrap();
        picking::project_world_to_screen(&cam, default_fov_y(), vw, vh, Vec3::ZERO).unwrap()
    }

    fn x_midpoint_press_screen(ctrl: &ViewportController) -> (f32, f32) {
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
        let (vw, vh) = ctrl.viewport_for_test().unwrap();
        let fov_y = default_fov_y();
        let scale = gizmo::gizmo_scale(&cam, Vec3::ZERO, fov_y, vh).unwrap();
        let midpoint = Vec3::new(scale * gizmo::dimensions::MIDPOINT_FRACTION, 0.0, 0.0);
        picking::project_world_to_screen(&cam, fov_y, vw, vh, midpoint).unwrap()
    }

    #[test]
    fn pointer_down_on_x_handle_enters_drag_state() {
        // The X translate hit region in the front-camera, 400x200 setup spans
        // screen-x [320, 350] at screen-y = 100. `X_CONE_PRESS` is a press
        // inside that region; only the cone tip is the active translate handle
        // (the shaft line is decorative).
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        match ctrl.input.gizmo_fsm().current() {
            GizmoState::Drag {
                op: DragOp::Translate { axis, .. },
                ..
            } => assert_eq!(axis, Axis::X),
            other => panic!("expected Drag(Translate, X), got {other:?}"),
        }
    }

    #[test]
    fn pointer_down_off_handle_does_not_enter_drag_state() {
        // A press far from any handle lands as a plain click candidate; the FSM
        // stays Idle.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(0.0, 0.0), no_mods());
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
    }

    #[test]
    fn pointer_down_on_handle_does_not_drag_outside_transform() {
        // The same press that grabs the X handle in Transform must not start a
        // drag while parked in a non-Transform section.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.set_section(WorkspaceSection::Supports);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        assert!(
            matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle),
            "the gizmo must not be grabbable outside the Transform section"
        );
    }

    #[test]
    fn leaving_transform_aborts_an_active_drag_and_reverts_it() {
        // Start a real drag in Transform, move it, then switch section: the
        // drag must abort and the transform revert to the captured original
        // (translation back to the origin).
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        assert!(matches!(
            ctrl.input.gizmo_fsm().current(),
            GizmoState::Drag { .. }
        ));
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        ctrl.set_section(WorkspaceSection::Slice);
        assert!(
            matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle),
            "leaving Transform must abort the in-progress drag"
        );
        assert_approx_eq!(f32, read_translation(&ctrl).x, 0.0, epsilon = 1e-4);
    }

    #[test]
    fn drag_translates_object_along_world_axis() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        // Press lands on the X cone tip; release stays on the cone (close
        // enough that the closest-point-on-axis delta is the controller's only
        // output to verify).
        let press = X_CONE_PRESS;
        let release = (X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1);
        let (vw, vh) = GIZMO_TEST_VIEWPORT;
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_ray = picking::ray_from_pixel(&cam, fov_y, vw, vh, press.0, press.1);
        let release_ray = picking::ray_from_pixel(&cam, fov_y, vw, vh, release.0, release.1);
        let press_anchor =
            gizmo::drag_math::closest_point_on_axis(&press_ray, Vec3::ZERO, Axis::X).unwrap();
        let release_anchor =
            gizmo::drag_math::closest_point_on_axis(&release_ray, Vec3::ZERO, Axis::X).unwrap();
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), no_mods());
        // The drag must have started; otherwise the move is a no-op.
        assert!(matches!(
            ctrl.input.gizmo_fsm().current(),
            GizmoState::Drag { .. }
        ));
        ctrl.on_pointer_move(ScreenPoint::new(release.0, release.1), no_mods());

        let actual = read_translation(&ctrl);
        assert_approx_eq!(f32, actual.x, expected.x, epsilon = 1e-4);
        assert_approx_eq!(f32, actual.y, expected.y, epsilon = 1e-4);
        assert_approx_eq!(f32, actual.z, expected.z, epsilon = 1e-4);
    }

    #[test]
    fn second_drag_move_recomposes_from_original_not_running_transform() {
        // 2 pointer moves during the same drag: the final transform must follow
        // the absolute cursor position relative to the press. A sum of
        // per-frame deltas agrees with it for a translate but not for a
        // rotation or a scale, which share the same drag pipeline, so this test
        // pins the absolute form.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        let press = X_CONE_PRESS;
        let mid = (X_CONE_PRESS.0 + 3.0, X_CONE_PRESS.1);
        let release = (X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1);
        let (vw, vh) = GIZMO_TEST_VIEWPORT;
        let cam = ctrl.camera.camera_cell().borrow().as_ref().unwrap().clone();
        let fov_y = default_fov_y();
        let press_anchor = gizmo::drag_math::closest_point_on_axis(
            &picking::ray_from_pixel(&cam, fov_y, vw, vh, press.0, press.1),
            Vec3::ZERO,
            Axis::X,
        )
        .unwrap();
        let release_anchor = gizmo::drag_math::closest_point_on_axis(
            &picking::ray_from_pixel(&cam, fov_y, vw, vh, release.0, release.1),
            Vec3::ZERO,
            Axis::X,
        )
        .unwrap();
        let expected = release_anchor - press_anchor;

        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), no_mods());
        ctrl.on_pointer_move(ScreenPoint::new(mid.0, mid.1), no_mods());
        ctrl.on_pointer_move(ScreenPoint::new(release.0, release.1), no_mods());

        let actual = read_translation(&ctrl);
        assert_approx_eq!(f32, actual.x, expected.x, epsilon = 1e-4);
    }

    #[test]
    fn pointer_up_after_drag_clears_state_and_keeps_translation() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        let mid_drag = read_translation(&ctrl);
        ctrl.on_pointer_up(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );

        // Drag state cleared.
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
        // Translation persists: the user committed the drag.
        let after = read_translation(&ctrl);
        assert_approx_eq!(f32, after.x, mid_drag.x, epsilon = 1e-6);
    }

    #[test]
    fn pointer_up_after_drag_pushes_one_undo_entry() {
        // A complete press -> move -> release gesture lands as 1 entry on the
        // document's undo stack, with the pre-press transform as the `before`.
        // A subsequent undo restores it.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        ctrl.on_pointer_up(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );

        assert!(ctrl.session.document().can_undo());
        assert!(!ctrl.session.document().can_redo());

        let after_drag = read_translation(&ctrl);
        assert!(after_drag.x.abs() > 1e-4);

        assert!(ctrl.session.document().undo().unwrap());
        let after_undo = read_translation(&ctrl);
        assert_approx_eq!(f32, after_undo.x, 0.0, epsilon = 1e-6);
        assert!(!ctrl.session.document().can_undo());
        assert!(ctrl.session.document().can_redo());

        assert!(ctrl.session.document().redo().unwrap());
        let after_redo = read_translation(&ctrl);
        assert_approx_eq!(f32, after_redo.x, after_drag.x, epsilon = 1e-6);
    }

    #[test]
    fn pointer_up_with_no_movement_pushes_no_undo_entry() {
        // Press -> release without a move tick: the transform is unchanged,
        // `push_edit` filters the no-op, and the stack stays empty.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());

        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn esc_mid_drag_pushes_no_undo_entry() {
        // Esc-to-cancel is not undoable: the user cancelled the gesture, so
        // nothing must land on the history stack.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        ctrl.on_clear_selection();

        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn esc_mid_drag_reverts_transform() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        // Object has moved.
        assert!(read_translation(&ctrl).x.abs() > 1e-4);

        // First Esc cascades into the drag-abort: transform reverts, FSM
        // returns to Idle, but the selection survives (Esc only drops 1 level
        // per press).
        ctrl.on_clear_selection();
        let reverted = read_translation(&ctrl);
        assert_approx_eq!(f32, reverted.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, reverted.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, reverted.z, 0.0, epsilon = 1e-6);
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
        assert_eq!(
            ctrl.selection(),
            Some(0),
            "first Esc only aborts the drag, selection is the next level of the cascade"
        );

        // A second Esc, with no drag and no keyboard focus, clears the
        // selection.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn close_scene_mid_drag_does_not_panic() {
        // The drag captures a node id, which becomes invalid when the scene is
        // dropped. `on_close_scene` must clear the FSM state without reverting
        // through the missing scene. The controller is wrapped in `Rc` with the
        // document listener registered, so the close-driven viewport reset runs
        // in the test.
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        install_single_object_at_origin(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        ctrl.on_close_scene();

        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
        assert_eq!(ctrl.selection(), None);
    }

    #[test]
    fn jitter_within_threshold_still_counts_as_a_click() {
        // A press -> release that moves about 2 pixels (trackpad jitter,
        // debounced finger lift) must still be treated as a click. The
        // threshold is 4 logical pixels.
        let ctrl = ViewportController::new_for_test();
        let (mut g, root) = graph_with_group_root();
        add_front_facing_triangle(&mut g, root, [0.0, 0.0, 0.0]);
        ctrl.install_graph_for_test(g);
        *ctrl.camera.camera_cell().borrow_mut() =
            Some(camera_looking_down_minus_z_at(Vec3::ZERO, 1.0));
        ctrl.set_viewport_for_test(200.0, 100.0);

        ctrl.on_pointer_down(ScreenPoint::new(100.0, 50.0), no_mods());
        ctrl.on_pointer_up(ScreenPoint::new(101.5, 51.5), no_mods());
        assert_eq!(ctrl.selection(), Some(0));
    }

    #[test]
    fn pressing_axis_cube_starts_a_per_axis_drag_with_or_without_shift() {
        // Shift no longer turns a cube drag uniform (the centre ball does
        // that): it only slows the drag down.
        for mods in [no_mods(), shift_held()] {
            let ctrl = ViewportController::new_for_test();
            install_single_object_at_origin(&ctrl);
            let press = x_midpoint_press_screen(&ctrl);
            ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), mods);
            match ctrl.input.gizmo_fsm().current() {
                GizmoState::Drag {
                    op: DragOp::ScaleAxis { axis, .. },
                    ..
                } => assert_eq!(axis, Axis::X),
                other => panic!("expected Drag(ScaleAxis, X), got {other:?}"),
            }
        }
    }

    #[test]
    fn pressing_the_centre_ball_scales_uniformly() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let pivot = pivot_press_screen(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(pivot.0, pivot.1), no_mods());
        assert!(matches!(
            ctrl.input.gizmo_fsm().current(),
            GizmoState::Drag {
                op: DragOp::ScaleUniform { .. },
                ..
            }
        ));
        ctrl.on_pointer_move(ScreenPoint::new(pivot.0 + 30.0, pivot.1), no_mods());
        let m = read_linear(&ctrl);
        assert!(m[0][0] > 1.1, "{m:?}");
        assert_approx_eq!(f64, m[0][0], m[1][1], epsilon = 1e-9);
        assert_approx_eq!(f64, m[0][0], m[2][2], epsilon = 1e-9);
        ctrl.on_pointer_up(ScreenPoint::new(pivot.0 + 30.0, pivot.1), no_mods());
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
    }

    #[test]
    fn shift_mid_drag_slows_it_down_without_a_jump() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 20.0, X_CONE_PRESS.1),
            no_mods(),
        );
        let full = read_translation(&ctrl).x;
        assert!(full > 0.0);
        // Shift goes down without moving: nothing moves.
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 20.0, X_CONE_PRESS.1),
            shift_held(),
        );
        assert_approx_eq!(f32, read_translation(&ctrl).x, full, epsilon = 1e-6);
        // Another 20 px with Shift moves a tenth as far as the first 20.
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 40.0, X_CONE_PRESS.1),
            shift_held(),
        );
        let fine = read_translation(&ctrl).x - full;
        assert_approx_eq!(f32, fine, full * 0.1, epsilon = full * 0.02);
    }

    #[test]
    fn ctrl_mid_drag_snaps_and_esc_still_restores_the_original() {
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), no_mods());
        let ctrl_held = PointerModifiers {
            control: true,
            ..no_mods()
        };
        ctrl.on_pointer_move(ScreenPoint::new(press.0 + 23.0, press.1), ctrl_held);
        let m = read_linear(&ctrl);
        let tenths = m[0][0] * 10.0;
        assert!(m[0][0] > 1.05, "{m:?}");
        assert_approx_eq!(f64, tenths, tenths.round(), epsilon = 1e-4);
        ctrl.on_clear_selection();
        let m = read_linear(&ctrl);
        assert_approx_eq!(f64, m[0][0], 1.0, epsilon = 1e-9);
    }

    /// Subscribes to the document and counts only `Mutated` events. Used by the
    /// routing tests below to filter out the `Installed`, `Cleared` and
    /// `SelectionChanged` traffic that `install_single_object_at_origin` and
    /// the other setup helpers generate.
    fn subscribe_mutated_counter(doc: &Rc<SceneDocument>) -> (Rc<Cell<usize>>, SceneSubscription) {
        let count = Rc::new(Cell::new(0_usize));
        let count_for_listener = count.clone();
        let sub = doc.subscribe(move |event| {
            if matches!(event, SceneEvent::Mutated) {
                count_for_listener.set(count_for_listener.get() + 1);
            }
        });
        (count, sub)
    }

    #[test]
    fn drag_move_fires_one_mutated_event_per_pointer_move() {
        // Press the X cone to enter Drag, then a single move tick that mutates
        // the object's transform must fire exactly 1 Mutated. Subscribing after
        // the press skips the SelectionChanged from
        // `install_single_object_at_origin`.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn pointer_hover_without_drag_fires_no_mutated_event() {
        // Pointer moves without a press can change the hover state, but they do
        // not mutate the scene, so no Mutated must fire.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
        ctrl.on_pointer_move(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 50.0, X_CONE_PRESS.1),
            no_mods(),
        );
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn gizmo_abort_with_drag_in_flight_fires_one_mutated_event() {
        // Press into Drag, subscribe, then abort the drag. The revert routes
        // through `apply_edit`, which fires Mutated exactly once.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        assert!(matches!(
            ctrl.input.gizmo_fsm().current(),
            GizmoState::Drag { .. }
        ));

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
        let aborted = ctrl.input.gizmo_fsm().abort(ctrl.session.document());
        assert!(aborted);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn gizmo_abort_with_no_drag_fires_no_mutated_event() {
        // No drag in flight: abort short-circuits before touching the document,
        // so no Mutated must fire.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));

        let (count, _sub) = subscribe_mutated_counter(ctrl.session.document());
        let aborted = ctrl.input.gizmo_fsm().abort(ctrl.session.document());
        assert!(!aborted);
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn drag_move_marks_redraw_owed_via_mutated_listener() {
        // End-to-end: with `register_document_listener` installed, a drag move
        // fires Mutated and the controller's listener marks the controller
        // dirty.
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        install_single_object_at_origin(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.clear_frame_state_for_test();
        assert!(!ctrl.redraw_owed_for_test());

        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 5.0, X_CONE_PRESS.1),
            no_mods(),
        );
        assert!(ctrl.redraw_owed_for_test());
    }

    #[test]
    fn set_build_volume_updates_field_and_marks_redraw_owed() {
        // The render path gates on `frame.redraw_owed`; without the dirty flag
        // the next `BeforeRendering` tick early-returns and the new envelope
        // only paints when something else (camera rotate, pointer move) happens
        // to mark the frame.
        let ctrl = ViewportController::new_for_test();
        let initial = ctrl.session.build_volume();
        let new_bv = BuildVolume::new(50.0, 50.0, 50.0);
        assert_ne!(initial, new_bv);
        ctrl.clear_frame_state_for_test();
        assert!(!ctrl.redraw_owed_for_test());

        ctrl.set_build_volume(new_bv);

        assert_eq!(ctrl.session.build_volume(), new_bv);
        assert!(ctrl.redraw_owed_for_test());
    }

    #[test]
    fn set_build_volume_is_idempotent_for_equal_values() {
        // Same value 2 times: the second call is a no-op and the frame stays
        // clean. A subsequent real change re-marks dirty.
        let ctrl = ViewportController::new_for_test();
        let bv = BuildVolume::new(80.0, 80.0, 120.0);
        ctrl.set_build_volume(bv);
        ctrl.clear_frame_state_for_test();

        ctrl.set_build_volume(bv);
        assert_eq!(ctrl.session.build_volume(), bv);
        assert!(!ctrl.redraw_owed_for_test());

        let bigger = BuildVolume::new(150.0, 80.0, 120.0);
        ctrl.set_build_volume(bigger);
        assert_eq!(ctrl.session.build_volume(), bigger);
        assert!(ctrl.redraw_owed_for_test());
    }

    #[test]
    fn gizmo_translate_drag_moves_past_plus_x_wall() {
        // The fixture's triangle has world.max.x = 0.5 with the installed
        // transform (an `Affine` equal to the identity). A build envelope whose
        // env.max.x = 0.5 leaves no room on +X, yet the drag still moves the
        // object out of the plate: the out-of-volume warning marks it, it
        // does not hold it back.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.session
            .set_build_volume(BuildVolume::new(1.0, 4.0, 4.0));

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 50.0, X_CONE_PRESS.1),
            no_mods(),
        );

        let actual = read_translation(&ctrl);
        assert!(actual.x > 0.1, "the drag moved {actual}");
        assert_approx_eq!(f32, actual.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, actual.z, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn gizmo_translate_drag_moves_an_object_already_past_both_walls() {
        // An object scaled wider than the plate crosses both X walls. Dragging
        // it either way along X still moves it.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        ctrl.session
            .set_build_volume(BuildVolume::new(0.2, 4.0, 4.0));

        ctrl.on_pointer_down(ScreenPoint::new(X_CONE_PRESS.0, X_CONE_PRESS.1), no_mods());
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 + 50.0, X_CONE_PRESS.1),
            no_mods(),
        );
        assert!(read_translation(&ctrl).x > 0.1, "moved away from the plate");
        ctrl.on_pointer_move(
            ScreenPoint::new(X_CONE_PRESS.0 - 50.0, X_CONE_PRESS.1),
            no_mods(),
        );
        assert!(read_translation(&ctrl).x < -0.1, "moved back across it");
    }
}

#[cfg(test)]
mod keyboard_focus_tests {
    use std::sync::Arc;

    use super::super::keyboard_drive::{ARROW_X_POS, ARROW_Y_POS, IntegrationCtx};
    use super::*;
    use crate::camera::OrbitCamera;
    use crate::gizmo::axis::Axis;
    use crate::scene_document::SceneEdit;
    use glam::Vec3;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_scene::{LeafPayload, Scene};

    /// Builds a loader-shape graph of 1 object: a single inline-mesh leaf under
    /// an identity-Group root.
    fn one_object_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        };
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

    #[test]
    fn focus_requires_an_existing_selection() {
        // No selection -> T is a no-op: `try_set_focus` refuses a focus without
        // a selection.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_gizmo_focus_translate();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::None);
    }

    #[test]
    fn focus_set_with_selection_takes_effect() {
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Translate);
        ctrl.on_gizmo_focus_rotate();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Rotate);
        ctrl.on_gizmo_focus_scale();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Scale);
    }

    #[test]
    fn switching_focus_resets_held_arrow_bitset() {
        // Holding +X under translate focus, then switching to rotate focus must
        // clear the held bit: the user expects a mode switch to release the
        // key.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        assert!(ctrl.input.kbd_drive().current_arrows().any_held());

        ctrl.on_gizmo_focus_rotate();
        assert!(
            !ctrl.input.kbd_drive().current_arrows().any_held(),
            "switching focus must clear held arrows so a held key from one mode \
             doesn't leak into another"
        );
    }

    #[test]
    fn arrow_press_without_focus_is_a_no_op() {
        // Without focus, an arrow code is ignored. The FocusScope on the Slint
        // side filters them out, but the Rust handler must not rely on that.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        assert!(!ctrl.input.kbd_drive().current_arrows().any_held());
    }

    #[test]
    fn arrow_release_clears_bit_even_with_focus_dropped() {
        // A release that arrives after focus was dropped (race with Esc) must
        // still clear the bit; otherwise the next focus session would inherit a
        // phantom held key.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        // Focus drops, for example through the Esc cascade.
        ctrl.on_clear_selection();
        // Stale release arrives.
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);
        assert!(!ctrl.input.kbd_drive().current_arrows().any_held());
    }

    fn translation_x(dx: f64) -> Transform3D {
        Transform3D::Affine(
            sc_mesh_core::transform::AffineTransform3D::from_translation([dx, 0.0, 0.0]),
        )
    }

    #[test]
    fn keyboard_burst_release_pushes_one_undo_entry() {
        // First arrow arms the burst; intervening transform mutation simulates
        // the integrator's ticks; release flushes the burst as a single undo
        // entry.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(2.0),
            })
            .unwrap();
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);

        assert!(ctrl.session.document().can_undo());
        assert!(ctrl.session.document().undo().unwrap());
        let after_undo = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
            .unwrap();
        assert_eq!(after_undo, Transform3D::Identity);
    }

    #[test]
    fn keyboard_burst_with_no_motion_pushes_no_entry() {
        // Press -> release without any integration tick: the scene transform
        // never changed, so the no-op filter of push_edit drops the entry.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);

        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn keyboard_burst_focus_change_flushes_pending_burst() {
        // Switching from translate to rotate mid-burst commits the current run
        // as a single undo entry, so the user's translate work is kept.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(3.0),
            })
            .unwrap();
        // User switches mode without releasing the key.
        ctrl.on_gizmo_focus_rotate();

        assert!(ctrl.session.document().can_undo());
        ctrl.session.document().undo().unwrap();
        let after_undo = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
            .unwrap();
        assert_eq!(after_undo, Transform3D::Identity);
    }

    #[test]
    fn keyboard_burst_two_keys_held_is_a_single_undo_entry() {
        // 2 arrows pressed and released independently still make a single
        // burst: only the first press arms the tracker, and only the release of
        // the last held key flushes it.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        ctrl.on_gizmo_arrow_pressed(ARROW_Y_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(1.0),
            })
            .unwrap();
        ctrl.on_gizmo_arrow_released(ARROW_X_POS);
        // The bitset still has Y_POS held, so the burst must not flush yet.
        assert!(!ctrl.session.document().can_undo());
        ctrl.on_gizmo_arrow_released(ARROW_Y_POS);
        assert!(ctrl.session.document().can_undo());
        // Exactly 1 entry: after 1 undo, nothing is left to undo.
        assert!(ctrl.session.document().undo().unwrap());
        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn keyboard_burst_dropped_on_scene_close() {
        // Scene close drops the burst without flushing: the node id it captured
        // belongs to the closed scene.
        let ctrl = Rc::new(ViewportController::new_for_test());
        ctrl.register_document_listener();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        let node_id = ctrl.session.document().selection_primary_node_id().unwrap();
        ctrl.session
            .document()
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation_x(1.0),
            })
            .unwrap();

        ctrl.on_close_scene();
        assert!(!ctrl.session.document().can_undo());
    }

    #[test]
    fn esc_cascade_drag_then_focus_then_selection() {
        // Each Esc drops 1 level of the cascade.
        let ctrl = ViewportController::new_for_test();
        ctrl.install_graph_for_test(one_object_graph());
        ctrl.on_select_by_index(0);
        ctrl.on_gizmo_focus_translate();
        // No drag is possible without a camera; simulate the mid-cascade state
        // by directly writing the gizmo cell.
        let node_id = ctrl
            .session
            .document()
            .selection_primary_node_id()
            .expect("test selected object 0");
        ctrl.input.gizmo_fsm().force_set(GizmoState::Drag {
            op: DragOp::Translate {
                axis: Axis::X,
                anchor: Vec3::ZERO,
            },
            original: Transform3D::Identity,
            pivot: Vec3::ZERO,
            node_id,
        });

        // Level 1 -> drag aborts; focus and selection survive.
        ctrl.on_clear_selection();
        assert!(matches!(ctrl.input.gizmo_fsm().current(), GizmoState::Idle));
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::Translate);
        assert_eq!(ctrl.selection(), Some(0));

        // Level 2 -> focus drops; selection survives.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.input.kbd_drive().current_focus(), KbdFocus::None);
        assert_eq!(ctrl.selection(), Some(0));

        // Level 3 -> selection clears.
        ctrl.on_clear_selection();
        assert_eq!(ctrl.selection(), None);
    }

    /// Builds a single object whose box has max.x = 1 under the identity
    /// transform, for envelope tests where the envelope is flush with the
    /// object's +X face, so any +X move takes it out of the plate.
    fn cube_object_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([-1.0, -1.0, 0.0]), Vertex::new([1.0, 1.0, 2.0])],
            faces: vec![],
        };
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

    /// Installs the cube object with a front-axis camera and a 400 x 200
    /// viewport, the gizmo tests' setup, so `world_per_pixel` has the same
    /// scale and the keyboard integrator's translation rate is comparable.
    fn install_for_integration(ctrl: &ViewportController) {
        ctrl.install_graph_for_test(cube_object_graph());
        let mut cam = OrbitCamera::new(Vec3::ZERO, 1.0);
        cam.set_axis_view(AxisView::Front);
        *ctrl.camera.camera_cell().borrow_mut() = Some(cam);
        ctrl.set_viewport_for_test(400.0, 200.0);
    }

    /// Drives `kbd_drive.integrate` 2 times with a real-time pause between the
    /// calls, so the second call sees a positive `dt`. The first call seeds
    /// `last_tick`; the second produces the actual integration step.
    fn integrate_two_ticks(ctrl: &ViewportController) {
        let _ = ctrl.input.kbd_drive().integrate(IntegrationCtx {
            document: ctrl.session.document(),
            camera: ctrl.camera.camera_cell(),
            viewport: ctrl.render.viewport(),
            selection_id: ctrl.session.document().selection_primary_node_id(),
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = ctrl.input.kbd_drive().integrate(IntegrationCtx {
            document: ctrl.session.document(),
            camera: ctrl.camera.camera_cell(),
            viewport: ctrl.render.viewport(),
            selection_id: ctrl.session.document().selection_primary_node_id(),
        });
    }

    #[test]
    fn keyboard_translate_focus_moves_past_plus_x_wall() {
        // Cube world AABB.max.x = 1.0 with identity. Envelope of (2, 4, 4)
        // gives env.max.x = 1.0 exactly: no room on +X. Holding +X under
        // Translate focus still moves the object out of the plate.
        let ctrl = ViewportController::new_for_test();
        install_for_integration(&ctrl);
        ctrl.on_select_by_index(0);
        ctrl.session
            .set_build_volume(BuildVolume::new(2.0, 4.0, 4.0));
        ctrl.on_gizmo_focus_translate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        integrate_two_ticks(&ctrl);

        let transform = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
            .unwrap();
        let translation = match transform {
            Transform3D::Identity => [0.0; 3],
            Transform3D::Affine(a) => *a.translation(),
        };
        assert!(
            translation[0] > 0.0,
            "expected the object to move past the +X wall, got {}",
            translation[0]
        );
    }

    #[test]
    fn keyboard_rotate_focus_unaffected_by_envelope() {
        // Same envelope-flush-with-cube setup. Rotate focus must ignore the
        // envelope: the linear part of the resulting transform reflects the
        // integrated rotation, although the composed transform also gets a
        // translation (a rotation about a pivot off the origin keeps the box
        // centroid in place by writing a translation into the affine form). The
        // test asserts that the linear part leaves the identity: the rotation
        // happened, and nothing undid it.
        let ctrl = ViewportController::new_for_test();
        install_for_integration(&ctrl);
        ctrl.on_select_by_index(0);
        ctrl.session
            .set_build_volume(BuildVolume::new(2.0, 4.0, 4.0));
        ctrl.on_gizmo_focus_rotate();

        ctrl.on_gizmo_arrow_pressed(ARROW_X_POS);
        integrate_two_ticks(&ctrl);

        let transform = ctrl
            .session
            .document()
            .with_scene(|s| {
                let id = s.leaf_id_at_index(0).expect("test fixture has a leaf");
                s.leaf(id).unwrap().local_transform()
            })
            .unwrap();
        match transform {
            Transform3D::Identity => panic!("rotate focus should have produced an affine"),
            Transform3D::Affine(a) => {
                let m = a.linear();
                // R_x(theta) maps (0, 1, 0) to (0, cos, sin) and (0, 0, 1) to
                // (0, -sin, cos). For the small theta of 1 integration step,
                // m[2][1] (the Y-into-Z coefficient) is sin(theta), which sets
                // the result apart from the identity matrix, where it is 0.
                assert!(
                    m[2][1].abs() > 1e-6,
                    "expected non-trivial rotation, got linear {:?}",
                    m
                );
            }
        }
    }
}

#[cfg(test)]
mod paste_clipboard_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_scene_edit::SceneFragment;

    fn triangle_clipboard() -> Clipboard {
        let mesh = Arc::new(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        });
        Clipboard::new(SceneFragment::from_leaf_snapshot(
            mesh,
            Transform3D::Identity,
        ))
    }

    #[test]
    fn paste_without_viewport_returns_false() {
        // No set_viewport_for_test: the render host's viewport is None, so the
        // controller bails before reaching the camera.
        let ctrl = ViewportController::new_for_test();
        let cb = triangle_clipboard();
        assert!(!ctrl.paste_clipboard_at_cursor(&cb));
    }

    #[test]
    fn paste_without_camera_returns_false() {
        let ctrl = ViewportController::new_for_test();
        ctrl.set_viewport_for_test(800.0, 600.0);
        // Neither on_scene_load_finished nor a paint framed a camera, so the
        // camera cell is still None.
        let cb = triangle_clipboard();
        assert!(!ctrl.paste_clipboard_at_cursor(&cb));
    }
}

/// A plate's length as its front label writes it: whole millimetres bare,
/// otherwise to a tenth.
fn plate_length(mm: f32) -> String {
    let tenths = (mm * 10.0).round();
    if tenths % 10.0 == 0.0 {
        format!("{}", tenths / 10.0)
    } else {
        format!("{:.1}", tenths / 10.0)
    }
}
