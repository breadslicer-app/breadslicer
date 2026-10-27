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
use std::rc::Rc;

use sc_mesh_core::transform::Transform3D;
use slint::ComponentHandle;
use slint::wgpu_30::wgpu;

use super::camera_controller::CameraController;
use super::gizmo_drag::DragOp;
use super::gizmo_drag::GizmoState;
use super::gizmo_frame_composer::GizmoFrameComposer;
use super::input_dispatch::{InputDispatch, PointerUpOutcome};
use super::keyboard_drive::KbdFocus;
use super::modal_transform::{ModalGesture, ModalOp};
use super::numeric_transform::{
    GestureKind, NumericEdit, Readout, cheap_readout, gesture_delta_text, readout,
};
use super::object_list::{build_object_list_names, push_object_list};
use super::render_host::RenderHost;
use super::session_state::SessionState;
use super::types::{FrameState, ScreenDelta, ScreenPoint};
use crate::MainWindow;
use crate::PointerModifiers;
use crate::WorkspaceSection;
use crate::build_volume::BuildVolume;
use crate::camera::AxisView;
use crate::clipboard;
use crate::clipboard::Clipboard;
use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::export_progress_panel::ExportProgressPanel;
use crate::gizmo;
use crate::gizmo::axis::Axis;
use crate::i18n::ctr;
use crate::mesh_health::needs_repair;
use crate::mesh_loader::start_load_with_ticket;
use crate::mesh_render::{DeviceLoss, RenderError, RendererInitError, SceneRender, StageRefusal};
use crate::pending_load::PendingLoad;
use crate::picking;
use crate::preferences_bus::PreferencesBus;
#[cfg(test)]
use crate::preferences_bus::PreferencesSnapshot;
use crate::print_file_preview;
use crate::save_progress_panel::SaveProgressPanel;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneEvent};
use crate::scene_loader::{PreparedScene, SceneLoader};
use crate::selection::{LeafId, Selection};
use sc_scene::SceneError;
use sc_sla_formats::{PrintPreviews, PrinterFileFormat};

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
}

/// A tool of a workspace tab that acts where the model is clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabTool {
    AddSupport,
    RemoveSupport,
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
        let loader = Rc::new(SceneLoader::new(ui, dialogs.clone(), device_loss));
        let save_progress = Rc::new(SaveProgressPanel::new(ui.as_weak()));
        let export_progress = Rc::new(ExportProgressPanel::new(ui.as_weak()));
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
            camera: CameraController::new(),
            render: RenderHost::new(ui),
            input: InputDispatch::new(),
            slice_view_key: Cell::new(None),
            plate_printer_name: RefCell::new(None),
            readout_owed: Cell::new(true),
            readout_base: Cell::new(None),
            overlay: RefCell::new(None),
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
            camera: CameraController::new(),
            render: RenderHost::new_for_test(),
            input: InputDispatch::new(),
            slice_view_key: Cell::new(None),
            plate_printer_name: RefCell::new(None),
            readout_owed: Cell::new(true),
            readout_base: Cell::new(None),
            overlay: RefCell::new(None),
        };
        // Most viewport tests exercise the Transform-section tooling (gizmo
        // drag, keyboard drive, delete, paste); the fixture starts in
        // Transform, as production does. Section-gating tests set the section
        // they need explicitly.
        controller.session.set_section(WorkspaceSection::Transform);
        controller
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
            .install_graph(graph, std::path::PathBuf::from("<test>"));
    }

    #[cfg(test)]
    pub(crate) fn clear_frame_state_for_test(&self) {
        self.render.clear_frame_state();
    }

    #[cfg(test)]
    pub(crate) fn redraw_owed_for_test(&self) -> bool {
        self.render.redraw_owed()
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
    pub(crate) fn viewport_for_test(&self) -> Option<(f32, f32)> {
        self.render.viewport().map(|vp| (vp.width(), vp.height()))
    }

    #[cfg(test)]
    pub(super) fn keyboard_focus_for_test(&self) -> KbdFocus {
        self.input.kbd_drive().current_focus()
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
        if !matches!(event, SceneEvent::SliceChanged | SceneEvent::DirtyChanged) {
            self.readout_owed.set(true);
        }
        match event {
            SceneEvent::Installed => self.on_scene_installed(),
            SceneEvent::Cleared => self.on_scene_cleared(),
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
            let name = self
                .session
                .document()
                .source_file_name()
                .unwrap_or_default();
            ui.set_document_name(name.into());
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

    /// Resets the viewport after a fresh scene install: the camera goes back to
    /// `None`, so the framing in `on_scene_load_finished` can set it, and the
    /// UI mirrors the new scene's object list and the cleared selection.
    fn on_scene_installed(&self) {
        self.input.modal.set(None);
        self.input.modal_revision.set(None);
        self.mirror_modal_to_ui();
        self.camera.reset();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orthographic_view(false);
        }
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
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orthographic_view(false);
        }
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
    pub(crate) fn set_section(&self, section: WorkspaceSection) {
        self.on_modal_transform_finish(false);
        if !self.session.set_section(section) {
            return;
        }
        if !self.allows_transform_gizmo() {
            self.cancel_transform_gestures();
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
            .is_some_and(|shown| std::sync::Arc::ptr_eq(shown, pieces))
    }

    /// Shows `pieces` over the scene until replaced (`None` clears them).
    pub(crate) fn set_preview_overlay(&self, pieces: Option<crate::mesh_render::Overlay>) {
        let changed = match (&*self.overlay.borrow(), &pieces) {
            (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            *self.overlay.borrow_mut() = pieces;
            self.mark_dirty();
        }
    }

    /// Repaints after the slice height slider moved, so the mesh clip follows
    /// the new layer; the paint reads the clip height from the current UI
    /// value.
    pub(crate) fn on_selected_layer_changed(&self) {
        self.mark_dirty();
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

    pub(crate) fn on_rotate(&self, delta: ScreenDelta) {
        if self.camera.on_rotate(delta, self.render.viewport()) {
            self.mark_dirty();
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

    pub(crate) fn on_toggle_projection(&self) {
        if let Some(cam) = self.camera.camera_cell().borrow_mut().as_mut() {
            cam.orthographic = !cam.orthographic;
            if let Some(ui) = self.ui.upgrade() {
                ui.set_orthographic_view(cam.orthographic);
            }
            self.mark_dirty();
        }
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
        if let Some(bounds) = bounds {
            if let Some(cam) = self.camera.camera_cell().borrow_mut().as_mut() {
                cam.frame_bounds(
                    glam::Vec3::from(bounds.min().coords()),
                    glam::Vec3::from(bounds.max().coords()),
                    crate::mesh_render::default_fov_y(),
                    vp.width() / vp.height().max(1.0),
                );
                self.mark_dirty();
            }
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
        self.session.document().clear_scene();
    }

    pub(crate) fn on_scene_load_finished(&self) {
        let Some(prepared) = self.session.loader().take_finished_load() else {
            return;
        };
        crate::breadcrumbs::note("load: finished");
        let PreparedScene {
            spatial,
            gpu_scene,
            project_payload,
            reloaded_slices,
            source_path,
            mesh_health,
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

        // Fires `SceneEvent::Installed`; the listener resets the camera to
        // `None` (so the bounds-based framing below sees a fresh slot), pushes
        // the new object list, and refreshes the other UI mirrors.
        self.session
            .document()
            .install_spatial(spatial, source_path, project_payload);

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
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mesh_needs_repair(needs_repair(mesh_health));
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
    /// can race with a scene reload.
    pub(crate) fn on_select_by_index(&self, idx: usize) {
        self.on_modal_transform_finish(false);
        // `leaf_id_at_index` returns `None` past the last reachable leaf.
        let new_sel = self
            .session
            .document()
            .with_scene(|spatial| spatial.leaf_id_at_index(idx).map(LeafId))
            .flatten()
            .map_or(Selection::None, Selection::One);
        // `set_selection` short-circuits on no-change and fires
        // `SceneEvent::SelectionChanged` only on actual transitions; the
        // listener mirrors the selection to the UI and calls `mark_dirty`.
        self.session.document().set_selection(new_sel);
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
        let next = match cur_idx {
            None => 0,
            Some(cur) => {
                let len_i = scene_len as i32;
                ((cur as i32 + step).rem_euclid(len_i)) as usize
            }
        };
        self.on_select_by_index(next);
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
        if !self.allows_layout_tools() {
            return;
        }
        if let Err(error) = self.session.document().delete_selected_leaf() {
            self.show_edit_rejected(error);
        }
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
            let label = gesture
                .map(|g| {
                    let axis = g.axis.map(|i| ["X", "Y", "Z"][i]).unwrap_or(match g.op {
                        ModalOp::Scale => "Uniform",
                        _ => "View",
                    });
                    format!("{} | {}", g.op.key(), axis)
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
            || self.repair_in_flight()
            || self.ui.upgrade().is_some_and(|ui| ui.get_loading())
            || matches!(self.input.gizmo_fsm().current(), GizmoState::Drag { .. })
        {
            return;
        }
        if self.input.modal.get().is_some_and(|g| g.op == op) {
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
        let gesture = document
            .with_scene(|spatial| {
                let camera_slot = self.camera.camera_cell().borrow();
                let camera = camera_slot.as_ref()?;
                let query = spatial.leaf(node_id)?;
                let pivot = gizmo::pivot_for_leaf(spatial, node_id)?;
                let fov = crate::mesh_render::default_fov_y();
                let screen = picking::project_world_to_screen(
                    camera,
                    fov,
                    viewport.width(),
                    viewport.height(),
                    pivot,
                )?;
                let inverse_view = camera.view_matrix().inverse();
                Some(ModalGesture {
                    node_id,
                    original: query.local_transform(),
                    parent: query.parent_world(),
                    pivot,
                    start: [x, y],
                    cursor: [x, y],
                    screen_pivot: [screen.0, screen.1],
                    right: inverse_view.transform_vector3(glam::Vec3::X).normalize(),
                    up: inverse_view.transform_vector3(glam::Vec3::Y).normalize(),
                    view_axis: inverse_view.transform_vector3(glam::Vec3::Z).normalize(),
                    world_per_pixel: camera.world_per_pixel_at(pivot, fov, viewport.height())?,
                    op,
                    axis: None,
                    last_angle: (y - screen.1).atan2(x - screen.0),
                    angle: 0.0,
                })
            })
            .flatten();
        self.input.modal.set(gesture);
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
        self.update_modal(ScreenPoint::new(x, y));
    }

    fn update_modal(&self, cursor: ScreenPoint) {
        if self.input.modal_revision.get() != Some(self.session.document().scene_revision()) {
            self.input.modal.set(None);
            self.input.modal_revision.set(None);
            self.mirror_modal_to_ui();
            return;
        }
        let Some(mut gesture) = self.input.modal.get() else {
            return;
        };
        let Some(proposed) = gesture.propose([cursor.x, cursor.y]) else {
            return;
        };
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

    pub(crate) fn on_modal_transform_finish(&self, confirm: bool) {
        let Some(gesture) = self.input.modal.take() else {
            return;
        };
        let document = self.session.document();
        if self.input.modal_revision.take() != Some(document.scene_revision()) {
            // A new document or externally committed edit owns the scene now.
            // Never restore a stale snapshot onto a possibly reused node id.
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

    /// Arms the translate keyboard focus (`T`); `R` and `S` arm rotate and
    /// scale the same way. Each needs a selection; without one, the press does
    /// nothing. A press of the focus already armed does nothing, so autorepeat
    /// does not reset the integrator's tick clock.
    pub(crate) fn on_gizmo_focus_translate(&self) {
        self.on_modal_transform_finish(false);
        self.apply_keyboard_focus(KbdFocus::Translate);
    }

    pub(crate) fn on_gizmo_focus_rotate(&self) {
        self.apply_keyboard_focus(KbdFocus::Rotate);
    }

    pub(crate) fn on_gizmo_focus_scale(&self) {
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
        if focus != KbdFocus::None && !self.allows_transform_gizmo() {
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

    pub(crate) fn on_pointer_down(&self, cursor: ScreenPoint, modifiers: PointerModifiers) {
        self.input.on_pointer_down(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
            modifiers,
            self.allows_transform_gizmo(),
        );
    }

    pub(crate) fn on_pointer_move(&self, cursor: ScreenPoint, modifiers: PointerModifiers) {
        // Track the cursor in every section so a later paste anchors on it; the
        // gizmo drag and hover run in Prepare and Transform.
        self.input.note_hover_cursor(cursor);
        self.tab_tool_hover(cursor);
        if self.input.modal.get().is_some() {
            self.update_modal(cursor);
            return;
        }
        if !self.allows_transform_gizmo() {
            return;
        }
        if self.input.on_pointer_move(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            self.session.build_volume(),
            cursor,
            modifiers,
        ) {
            self.mark_dirty();
        }
    }

    pub(crate) fn on_pointer_up(&self, cursor: ScreenPoint, _modifiers: PointerModifiers) {
        match self.input.on_pointer_up(self.session.document(), cursor) {
            PointerUpOutcome::DragCommitted => self.mark_dirty(),
            PointerUpOutcome::Click(cursor) => {
                if !self.support_tool_click(cursor) {
                    self.run_pick_at(cursor);
                }
            }
            PointerUpOutcome::Nothing => (),
        }
    }

    /// The tab tool armed in the current section, if any.
    fn tab_tool(&self, ui: &MainWindow) -> Option<TabTool> {
        match self.session.section() {
            WorkspaceSection::Hollow if ui.get_hollow_drain_tool() => Some(TabTool::Drain),
            WorkspaceSection::Supports => match ui.get_support_tool() {
                1 => Some(TabTool::AddSupport),
                2 => Some(TabTool::RemoveSupport),
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
        let ray = {
            let camera_slot = self.camera.camera_cell().borrow();
            picking::ray_from_pixel(
                camera_slot.as_ref()?,
                crate::mesh_render::default_fov_y(),
                vp.width(),
                vp.height(),
                cursor.x,
                cursor.y,
            )
        };
        self.session
            .document()
            .with_scene(|spatial| {
                let is_supports = |id: sc_scene::NodeId| {
                    spatial
                        .graph()
                        .leaf_mesh_arc(id)
                        .is_some_and(|m| sc_geom::df_supports::is_support_mesh(&m))
                };
                let (id, t) = picking::pick_object_where(&ray, spatial, |id| {
                    is_supports(id) == (tool == TabTool::RemoveSupport)
                })?;
                let normal = picking::ray_hit_normal(&ray, &spatial.leaf(id)?)
                    .map_or(glam::Vec3::Z, |(_, n)| n);
                Some((id, ray.origin + ray.direction * t, normal))
            })
            .flatten()
    }

    /// A click with a tab tool (the Supports tab's Add or Delete, the Hollow
    /// tab's drain): hands the world point under the cursor to its driver.
    /// The drain tool selects the model it hit first. Returns whether the
    /// click was the tool's (a miss with a tool on is still the tool's).
    fn support_tool_click(&self, cursor: ScreenPoint) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        let Some(tool) = self.tab_tool(&ui) else {
            return false;
        };
        // The preview showed what this click does; the edit replaces it.
        self.set_preview_overlay(None);
        if let Some((id, p, n)) = self.tab_tool_hit(tool, cursor) {
            if tool == TabTool::Drain {
                self.session
                    .document()
                    .set_selection(Selection::One(LeafId(id)));
                ui.invoke_hollow_drain_clicked(p.x, p.y, p.z, n.x, n.y, n.z);
            } else {
                ui.invoke_support_tool_clicked(p.x, p.y, p.z);
            }
            self.mark_dirty();
        }
        true
    }

    /// The cursor moved with no button down: an armed tab tool previews what
    /// a click there would do (its driver draws it, debounced); off the
    /// model, or with no tool, the preview goes.
    fn tab_tool_hover(&self, cursor: ScreenPoint) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(tool) = self.tab_tool(&ui) else {
            return;
        };
        let hit = self.tab_tool_hit(tool, cursor);
        let (p, n) = hit.map_or((glam::Vec3::ZERO, glam::Vec3::Z), |(_, p, n)| (p, n));
        if tool == TabTool::Drain {
            ui.invoke_hollow_drain_hover(p.x, p.y, p.z, n.x, n.y, n.z, hit.is_some());
        } else {
            ui.invoke_support_tool_hover(p.x, p.y, p.z, hit.is_some());
        }
    }

    /// The armed tab tool changed (or the tab did): a preview of the old tool
    /// no longer applies.
    pub(crate) fn on_tab_tools_changed(&self) {
        self.set_preview_overlay(None);
    }

    /// Clears the dispatcher's cached hover cursor when the cursor leaves the
    /// viewport, so the paste path falls back to the viewport-centre placement.
    pub(crate) fn on_pointer_leave(&self) {
        self.input.on_pointer_leave();
        self.set_preview_overlay(None);
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_hollow_drain_hover(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, false);
        }
    }

    /// Latches a right-button press position. The release path classifies the
    /// gesture as click (context menu) or drag (camera orbit) by comparing
    /// displacement against the click threshold.
    pub(crate) fn on_right_pointer_down(&self, x: f32, y: f32) {
        self.input.on_right_pointer_down(ScreenPoint { x, y });
    }

    /// Classifies a right-button release. `Some(press_origin)` when the gesture
    /// stayed within the click threshold (caller opens the context menu at that
    /// pixel); `None` for a drag the orbit gesture already consumed or a
    /// release without a matching press.
    pub(crate) fn on_right_pointer_up(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        self.input
            .on_right_pointer_up(ScreenPoint { x, y })
            .map(|p| (p.x, p.y))
    }

    /// Runs the picker at `(x, y)` without changing the document's selection.
    /// Returns the hit leaf's `NodeId`, or `None` on a miss, with no scene
    /// loaded, or with no camera ready.
    pub(crate) fn pick_node_at(&self, x: f32, y: f32) -> Option<sc_scene::NodeId> {
        self.input
            .pick_at(
                self.session.document(),
                self.camera.camera_cell(),
                self.render.viewport(),
                ScreenPoint { x, y },
            )
            .flatten()
    }

    /// Runs the picker at `cursor` through InputDispatch (which builds the ray
    /// and runs `pick_object`) and applies the result through the document's
    /// selection setters: a hit selects its leaf, a miss runs the Esc cascade.
    fn run_pick_at(&self, cursor: ScreenPoint) {
        let Some(pick) = self.input.pick_at(
            self.session.document(),
            self.camera.camera_cell(),
            self.render.viewport(),
            cursor,
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
        let Some(fragment_aabb) = clipboard::fragment_world_aabb(clipboard.fragment()) else {
            return false;
        };
        let Some(obstacles) = self
            .session
            .document()
            .with_scene(clipboard::placement::obstacle_aabbs_for_scene)
        else {
            return false;
        };
        let translation = clipboard::placement::paste_translation_for_fragment(
            &fragment_aabb,
            [world_x as f64, world_y as f64],
            floor_z,
            clipboard.cascade_count(),
            &obstacles,
        );
        match self
            .session
            .document()
            .paste_fragment(clipboard.fragment(), translation)
        {
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
        ui.set_selection_pos_x(r.position[0] as f32);
        ui.set_selection_pos_y(r.position[1] as f32);
        ui.set_selection_pos_z(r.position[2] as f32);
        ui.set_selection_rot_x(r.rotation_deg[0] as f32);
        ui.set_selection_rot_y(r.rotation_deg[1] as f32);
        ui.set_selection_rot_z(r.rotation_deg[2] as f32);
        ui.set_selection_scale(r.scale_pct as f32);
    }

    /// The viewport aids of a gesture in flight (BS-69): what it has done so
    /// far (distance, degrees, scale) in the chip, and, for a modal G/R/S
    /// locked to an axis, a line along that axis through the object in the
    /// axis colour, as Blender draws it. Cleared when no gesture runs.
    fn refresh_gesture_overlay(&self, ui: &MainWindow) {
        let document = self.session.document();
        let modal = self.input.modal.get();
        let drag = match self.input.gizmo_fsm().current() {
            GizmoState::Drag {
                op,
                original,
                node_id,
                ..
            } => Some((op, original, node_id)),
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
            (None, Some((op, original, node_id))) => {
                let (kind, axis) = match op {
                    DragOp::Translate { axis, .. } => (GestureKind::Move, axis),
                    DragOp::Rotate { axis, .. } => (GestureKind::Rotate, axis),
                    DragOp::ScaleAxis { axis, .. } => (GestureKind::Scale, axis),
                };
                (
                    node_id,
                    original,
                    kind,
                    Some(match axis {
                        Axis::X => 0,
                        Axis::Y => 1,
                        Axis::Z => 2,
                    }),
                    None,
                )
            }
            (None, None) => {
                ui.set_gesture_readout(slint::SharedString::default());
                ui.set_axis_guide_visible(false);
                return;
            }
        };
        let text = document
            .with_scene(|spatial| {
                let query = spatial.leaf(node)?;
                let parent = parent_known.unwrap_or_else(|| query.parent_world());
                Some(gesture_delta_text(
                    kind,
                    parent.compose(&local_before),
                    query.world_transform(),
                    axis,
                ))
            })
            .flatten()
            .unwrap_or_default();
        ui.set_gesture_readout(text.into());

        // The axis line: modal gestures carry the screen pivot and the
        // camera's screen axes.
        let guide = modal.and_then(|g| {
            let k = g.axis?;
            let world_axis = [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z][k];
            let dir = glam::Vec2::new(world_axis.dot(g.right), -world_axis.dot(g.up));
            (dir.length() > 1e-3).then(|| (k, g.screen_pivot, dir.normalize()))
        });
        match guide {
            Some((k, [px, py], dir)) => {
                let reach = 4000.0;
                let (a, b) = (
                    glam::Vec2::new(px, py) - dir * reach,
                    glam::Vec2::new(px, py) + dir * reach,
                );
                ui.set_axis_guide_commands(format!("M {} {} L {} {}", a.x, a.y, b.x, b.y).into());
                let [r, g, b] = [[235, 80, 80], [90, 205, 110], [90, 140, 255]][k];
                ui.set_axis_guide_color(slint::Color::from_rgb_u8(r, g, b).into());
                ui.set_axis_guide_visible(true);
            }
            None => ui.set_axis_guide_visible(false),
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

    pub(crate) fn handle_rendering_setup(&self, device: wgpu::Device, queue: wgpu::Queue) {
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
        if let Some(path) = self.session.pending_load().take()
            && let Some(ticket) = self.session.loader().begin_load()
        {
            start_load_with_ticket(
                ticket,
                path,
                self.session.loader(),
                self.session.build_volume(),
            );
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
        if let Some(path) = self.session.pending_load().take()
            && let Some(ticket) = self.session.loader().begin_load()
        {
            start_load_with_ticket(
                ticket,
                path,
                self.session.loader(),
                self.session.build_volume(),
            );
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
            self.session.build_volume(),
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
    fn reconcile_slice_view_image(
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
                .session
                .document()
                .with_slice_layer(layer, |l| {
                    crate::slice_cap::build_cap_raster(l, build_volume)
                })
                .flatten()
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
        if !self.render.frame().redraw_owed() {
            return;
        }

        let mut renderer_slot = self.render.renderer_cell().borrow_mut();
        let Some(renderer) = renderer_slot.as_mut() else {
            return;
        };
        renderer.set_overlay(self.overlay.borrow().as_ref());

        let Some(ui) = self.ui.upgrade() else {
            return;
        };
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
        let cap_key = clip_z.map(|_| {
            (self.session.document().slice_generation() << 20) ^ u64::from(selected_layer)
        });
        let mut build_cap = || {
            self.session
                .document()
                .with_slice_layer(selected_layer, |layer| {
                    crate::slice_cap::build_cap_raster(layer, &build_volume)
                })
                .flatten()
        };
        match renderer.update_cap(cap_key, &mut build_cap) {
            Ok(()) | Err(RenderError::DeviceLost) => {}
            Err(err) => self.session.loader().note_render_failure(&err),
        }

        // The word on the plate's front tab and the printer's name under it,
        // keyed on the text so a language or printer change rebuilds it and an
        // orbit does not.
        let front_word = ctr!("MeshViewer", "FRONT");
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
                // The selection highlight stays visible in every section
                // (selection is live everywhere). The gizmo is a transform
                // tool, so its handles compose in Prepare and Transform:
                // only the selection handed to the composer is gated, and the
                // object tint keeps the full selection.
                let gizmo_selection = if self.allows_transform_gizmo() {
                    selection_node_id
                } else {
                    None
                };
                let gizmo_frame = GizmoFrameComposer::compose(
                    &self.input,
                    spatial_for_render,
                    camera,
                    gizmo_selection,
                    h_logical,
                );

                renderer.render(
                    Some(SceneRender {
                        graph: spatial_for_render.graph(),
                        selection: selection_node_id,
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

    pub(crate) fn handle_before_rendering(&self) {
        self.refresh_readout();
        if !self.on_scene_change() {
            return;
        }
        self.on_paint();
    }
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
    fn pressing_axis_cube_with_shift_held_starts_proportional_drag() {
        // Pressing the per-axis midpoint cube while Shift is already held
        // captures `proportional = true` at once: the first move applies the
        // uniform formula, with no toggle.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), shift_held());
        match ctrl.input.gizmo_fsm().current() {
            GizmoState::Drag {
                op: DragOp::ScaleAxis {
                    axis, proportional, ..
                },
                ..
            } => {
                assert_eq!(axis, Axis::X);
                assert!(
                    proportional,
                    "proportional must capture from press-time Shift state"
                );
            }
            other => panic!("expected Drag(ScaleAxis, X, proportional=true), got {other:?}"),
        }
    }

    #[test]
    fn pressing_axis_cube_without_shift_starts_per_axis_drag() {
        // The inverse case: no Shift -> per-axis (proportional = false). This
        // pins the default, so a capture of stale modifier state fails here.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);
        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), no_mods());
        match ctrl.input.gizmo_fsm().current() {
            GizmoState::Drag {
                op: DragOp::ScaleAxis { proportional, .. },
                ..
            } => assert!(
                !proportional,
                "no Shift at press -> proportional must be false"
            ),
            other => panic!("expected Drag(ScaleAxis, .., proportional=false), got {other:?}"),
        }
    }

    #[test]
    fn shift_toggle_during_axis_scale_flips_to_uniform_formula() {
        // Press without Shift -> per-axis drag, move to grow X. Then press
        // Shift on a subsequent move: re-anchor (running transform preserved).
        // A *further* move with Shift held exercises the uniform formula: its
        // `diag(f, f, f)` composes onto the captured running transform, so Y
        // and Z both pick up the same `f` factor while X picks up an extra `f`
        // on top of the per-axis stretch baked in by the first move.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), no_mods());
        let move_no_shift = (press.0 + 20.0, press.1);
        ctrl.on_pointer_move(
            ScreenPoint::new(move_no_shift.0, move_no_shift.1),
            no_mods(),
        );
        let m_axis = read_linear(&ctrl);
        assert!(
            m_axis[0][0] > 1.0 + 1e-3,
            "X should have grown, got m[0][0]={}",
            m_axis[0][0]
        );
        assert_approx_eq!(f64, m_axis[1][1], 1.0, epsilon = 1e-6);
        assert_approx_eq!(f64, m_axis[2][2], 1.0, epsilon = 1e-6);

        // Shift toggles on at the same cursor position and re-anchors without
        // changing the running transform.
        let toggle = (move_no_shift.0, move_no_shift.1);
        ctrl.on_pointer_move(ScreenPoint::new(toggle.0, toggle.1), shift_held());

        // Move further with Shift held: the uniform formula runs against the
        // re-anchored press at `toggle`.
        let further = (toggle.0 + 20.0, toggle.1);
        ctrl.on_pointer_move(ScreenPoint::new(further.0, further.1), shift_held());
        let m_uniform = read_linear(&ctrl);

        // Uniform formula -> Y and Z scale by the same factor.
        assert_approx_eq!(f64, m_uniform[1][1], m_uniform[2][2], epsilon = 1e-4);
        // Both moved away from 1.0 (the formula did something).
        assert!(
            (m_uniform[1][1] - 1.0).abs() > 1e-3,
            "Y should have scaled under uniform formula, got m[1][1]={}",
            m_uniform[1][1]
        );
        // X carries the per-axis stretch on top of the uniform factor, so it
        // differs from Y and Z.
        assert!(
            (m_uniform[0][0] - m_uniform[1][1]).abs() > 1e-3,
            "X should differ from Y under uniform-after-per-axis: m[0][0]={} m[1][1]={}",
            m_uniform[0][0],
            m_uniform[1][1]
        );
    }

    #[test]
    fn shift_toggle_does_not_snap_running_transform() {
        // Re-anchoring on a Shift toggle must preserve the running transform on
        // the toggling frame: the scene's transform immediately after the
        // toggle move equals the running transform from immediately before.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);

        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), no_mods());
        let move1 = (press.0 + 20.0, press.1);
        ctrl.on_pointer_move(ScreenPoint::new(move1.0, move1.1), no_mods());
        let before_toggle = read_linear(&ctrl);
        let translation_before = read_translation(&ctrl);

        // Same cursor position, Shift toggles on. Re-anchor must preserve the
        // running transform; a toggle without cursor motion is the simplest
        // case in which the transform could jump.
        ctrl.on_pointer_move(ScreenPoint::new(move1.0, move1.1), shift_held());
        let after_toggle = read_linear(&ctrl);
        let translation_after = read_translation(&ctrl);

        for r in 0..3 {
            for c in 0..3 {
                assert_approx_eq!(f64, after_toggle[r][c], before_toggle[r][c], epsilon = 1e-4);
            }
        }
        assert_approx_eq!(
            f32,
            translation_after.x,
            translation_before.x,
            epsilon = 1e-4
        );
        assert_approx_eq!(
            f32,
            translation_after.y,
            translation_before.y,
            epsilon = 1e-4
        );
        assert_approx_eq!(
            f32,
            translation_after.z,
            translation_before.z,
            epsilon = 1e-4
        );
    }

    #[test]
    fn shift_toggle_off_returns_to_per_axis_formula() {
        // Press with Shift held -> uniform. Move once. Release Shift on the
        // next move (re-anchor again) -> per-axis formula should resume on
        // subsequent moves: only X scales relative to the new anchor, while Y
        // and Z stay at the value frozen at the toggle.
        let ctrl = ViewportController::new_for_test();
        install_single_object_at_origin(&ctrl);
        let press = x_midpoint_press_screen(&ctrl);

        // Start uniform.
        ctrl.on_pointer_down(ScreenPoint::new(press.0, press.1), shift_held());
        let move1 = (press.0 + 20.0, press.1);
        ctrl.on_pointer_move(ScreenPoint::new(move1.0, move1.1), shift_held());

        // Release Shift, re-anchor. Frozen Y and Z factors persist.
        ctrl.on_pointer_move(ScreenPoint::new(move1.0, move1.1), no_mods());
        let frozen = read_linear(&ctrl);

        // Move further along X: only m[0][0] should change.
        let move2 = (move1.0 + 10.0, move1.1);
        ctrl.on_pointer_move(ScreenPoint::new(move2.0, move2.1), no_mods());
        let after = read_linear(&ctrl);

        assert!(
            (after[0][0] - frozen[0][0]).abs() > 1e-3,
            "X should have scaled further: frozen={} after={}",
            frozen[0][0],
            after[0][0]
        );
        assert_approx_eq!(f64, after[1][1], frozen[1][1], epsilon = 1e-4);
        assert_approx_eq!(f64, after[2][2], frozen[2][2], epsilon = 1e-4);
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
    fn gizmo_translate_drag_clamps_at_plus_x_wall() {
        // The fixture's triangle has world.max.x = 0.5 with the installed
        // transform (an `Affine` equal to the identity). A build envelope whose
        // env.max.x = 0.5 leaves no room on +X: any positive drag delta must
        // clamp to 0. Without the clamp wiring, the same press and move would
        // translate the object along +X (covered by
        // `drag_translates_object_along_world_axis`).
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
        assert_approx_eq!(f32, actual.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, actual.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, actual.z, 0.0, epsilon = 1e-6);
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
    /// transform, for clamp tests where the envelope is flush with the object's
    /// +X face, so any +X delta clamps to 0.
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
            build_volume: ctrl.session.build_volume(),
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = ctrl.input.kbd_drive().integrate(IntegrationCtx {
            document: ctrl.session.document(),
            camera: ctrl.camera.camera_cell(),
            viewport: ctrl.render.viewport(),
            selection_id: ctrl.session.document().selection_primary_node_id(),
            build_volume: ctrl.session.build_volume(),
        });
    }

    #[test]
    fn keyboard_translate_focus_clamps_at_plus_x_wall() {
        // Cube world AABB.max.x = 1.0 with identity. Envelope of (2, 4, 4)
        // gives env.max.x = 1.0 exactly: no room on +X. Holding +X under
        // Translate focus must leave the object where it is across as many
        // ticks as we run.
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
            translation[0] <= f64::EPSILON,
            "expected translation.x clamped to 0 against +X wall, got {}",
            translation[0]
        );
    }

    #[test]
    fn keyboard_rotate_focus_unaffected_by_envelope() {
        // Same envelope-flush-with-cube setup. Rotate focus must ignore the
        // clamp: the linear part of the resulting transform reflects the
        // integrated rotation, although the composed transform also gets a
        // translation (a rotation about a pivot off the origin keeps the box
        // centroid in place by writing a translation into the affine form). The
        // test asserts that the linear part leaves the identity: the rotation
        // happened, and the clamp did not undo it.
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
