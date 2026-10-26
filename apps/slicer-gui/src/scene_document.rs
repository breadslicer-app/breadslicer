/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level model for the loaded scene.
//!
//! Owns the [`SceneSpatial`] (which in turn owns the [`Scene`]), the source
//! path, the selection, and the undo / redo command stacks. Consumers like the
//! viewport controller and the object-list panel observe the document instead
//! of holding their own copies.
//!
//! Mutation goes through 1 path: [`SceneDocument::apply_edit`]. It accepts a
//! typed [`SceneEdit`] from [`sc_scene_edit`] and routes through
//! [`SceneSpatial::apply_edit`], which keeps spatial caches in sync with the
//! scene state. There is no closure-based mutation entry point: the cache layer
//! would not be able to inspect a closure to decide what to invalidate.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_print_primitives::{MaterialId, OutputFormatBinding, PositiveFiniteMm, PrinterId};
use sc_project_formats::{EmbeddedProfile, PlateSlices, ProjectPayload};
use sc_scene::Scene;
use sc_scene::node::{LeafAttachment, LeafPayload, NodeKind};
use sc_scene::{MeshResourceId, NodeId, SceneError};
use sc_scene_edit::SceneFragment;
use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::profile::LayerSchedule;

pub(crate) use sc_scene_edit::SceneEdit;

use crate::print_selection::{PrintDefaults, PrintSelection};
use crate::selection::{Selection, SelectionState};
use crate::slice_result::{SliceParams, SliceResult};

/// An opaque marker of which scene is installed and of its committed-edit
/// state, taken by [`SceneDocument::scene_revision`] and compared by
/// [`SceneDocument::slice_geometry_changed`] to tell whether the geometry
/// changed between 2 moments (producing a slice and a later Slice press). Equal
/// markers prove the committed geometry matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SceneRevision {
    /// Which installed scene. 2 freshly loaded scenes both sit at an empty undo
    /// stack, so the edit head alone cannot tell them apart.
    scene: u64,
    /// Committed-edit head, the identity the save point also tracks.
    top_edit: Option<u64>,
}

impl SceneRevision {
    /// Builds a marker for `top_edit` within an arbitrary installed scene.
    #[cfg(test)]
    pub(crate) fn from_top_edit_id(top_edit: Option<u64>) -> Self {
        Self { scene: 0, top_edit }
    }
}

/// The document state a save was prepared from, rechecked when the write
/// completes. The write runs off the UI thread while the event loop stays live,
/// so all 3 parts can move on in between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SaveStamp {
    /// Which installed scene. 2 freshly loaded scenes both sit at an empty undo
    /// stack, so the edit head alone cannot tell them apart.
    scene: u64,
    /// Committed-edit head, the identity the save point also tracks.
    top_edit: Option<u64>,
    /// Number of project-level changes so far. The print choices never reach
    /// the undo stack.
    project_state: u64,
}

/// What [`SceneDocument::record_project_save_for`] did with a completed write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveRecorded {
    /// The document still held what was written; it now points at the target
    /// and reads clean.
    Clean,
    /// The same scene, moved on while the write ran. The target is recorded,
    /// and the document stays dirty for the changes the file does not contain.
    Drifted,
    /// The document holds a different scene now, so nothing was recorded
    /// against it.
    Replaced,
}

/// Maximum number of entries kept on each of the undo and redo stacks. On
/// overflow the oldest entry (front of the deque) is discarded.
pub(crate) const UNDO_STACK_CAP: usize = 256;

/// Coarse-grained dispatch hint for document listeners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SceneEvent {
    /// A scene has just been installed (replacing whatever was there).
    /// Selection has been reset to `None` and `source_path` is set.
    Installed,
    /// The scene has been removed; no scene is currently loaded.
    Cleared,
    /// Selection has changed without the scene itself changing.
    SelectionChanged,
    /// The scene's contents were mutated through [`SceneDocument::apply_edit`]
    /// (also the path used internally by `undo` and `redo`) WITHOUT changing
    /// arena membership. Fires for transform-only edits (gizmo drag,
    /// keyboard-driven arrow nudges, etc.).
    Mutated,
    /// A structural mutation landed: a leaf was added or removed, or a mesh
    /// resource was registered or removed. The GPU scene's per-leaf model
    /// buffer and the spatial caches need a re-upload to mirror the new arena
    /// shape, so listeners schedule that on top of the usual redraw.
    Restructured,
    /// The held slice result changed: a run finished, or a reopened project's
    /// revalidated slices landed. The scene itself is untouched; listeners that
    /// mirror slice state to the UI react here.
    SliceChanged,
    /// The undo or redo stack changed (push, undo, redo, or install/clear when
    /// the stacks were non-empty).
    UndoStackChanged,
    /// The value of [`SceneDocument::is_dirty`] flipped. Fired only on actual
    /// transitions (a push onto an already-dirty document stays silent), after
    /// the operation's other events.
    DirtyChanged,
}

type Listener = Rc<dyn Fn(SceneEvent)>;

/// A committed entry on the undo/redo stacks: the edit plus the identity the
/// dirty tracking compares against the save point. An entry keeps its id when
/// undo/redo moves it between the stacks, so "the same edit is on top again" is
/// detectable after an undo + redo round trip. Ids are never reused within a
/// document.
#[derive(Clone, Debug)]
struct HistoryEntry {
    id: u64,
    edit: SceneEdit,
}

/// What the document was loaded as: a project or a mesh. It decides whether
/// "Save" may overwrite that file in place (a project) or must go through "Save
/// As".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentOrigin {
    PlainMesh,
    Project,
}

/// Splits a project payload into the scene's own choices and the embedded
/// profile documents it carries.
fn split_payload(payload: ProjectPayload) -> (PrintSelection, Vec<EmbeddedProfile>) {
    let ProjectPayload {
        printer,
        resin,
        first_layer_height,
        output_format,
        embedded,
    } = payload;
    (
        PrintSelection {
            printer,
            resin,
            first_layer_height,
            output_format,
        },
        embedded,
    )
}

pub(crate) struct SceneDocument {
    spatial: RefCell<Option<SceneSpatial>>,
    /// Filesystem path the loaded scene came from, updated by a successful
    /// "Save As". Used to derive default export/save filenames and as the
    /// "Save" overwrite target. Cleared in lockstep with `spatial`.
    source_path: RefCell<Option<PathBuf>>,
    /// Kind of the file at `source_path`: set on install from whether the file
    /// carried project data, and flipped to `Project` by
    /// [`Self::record_project_save_for`].
    origin: Cell<DocumentOrigin>,
    /// The scene's own print choices.
    print_selection: RefCell<PrintSelection>,
    /// Embedded profile documents carried by the source file or by the latest
    /// save. Held opaquely.
    embedded_profiles: RefCell<Vec<EmbeddedProfile>>,
    /// Currently-selected object. Reset to `Selection::None` whenever the scene
    /// is installed or cleared: a new scene has its own object indices, so a
    /// prior selection is no longer meaningful.
    selection: SelectionState,
    /// Registered listeners. Cloned out of the cell before dispatch so a
    /// listener can re-enter `subscribe` without re-borrowing.
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_listener_id: Cell<u64>,
    /// Reversible edits the user can roll back via Edit -> Undo. Most recent
    /// edit at the back; capped at `UNDO_STACK_CAP`.
    undo_stack: RefCell<VecDeque<HistoryEntry>>,
    /// Edits that were rolled back by `undo` and can be replayed by `redo`.
    /// Cleared whenever a fresh edit lands via `push_edit`: new history
    /// invalidates the redo trail.
    redo_stack: RefCell<VecDeque<HistoryEntry>>,
    /// Id source for [`HistoryEntry`]; the first committed edit gets 1.
    next_edit_id: Cell<u64>,
    /// Dirty-tracking save point: the id of the undo stack's top entry when the
    /// document was last saved, `None` when it was saved (or freshly installed)
    /// with an empty stack. The top matching the save point proves the content
    /// matches the saved state: eviction at the cap only forgets undo ability,
    /// it never mutates the scene, and ids are unique.
    save_point: Cell<Option<u64>>,
    /// Set when the cap evicts an entry from the undo stack; reset whenever a
    /// save point is recorded (save, install, clear). An empty stack matching
    /// an empty-stack save point only proves the content is back at the saved
    /// state when nothing was evicted in between: evicted edits stay applied to
    /// the scene with no way to undo them, so after an eviction the
    /// empty-vs-empty comparison must read dirty.
    evicted_since_save: Cell<bool>,
    /// Save-dirty contribution from project-level state that does not live on
    /// the undo stack: the scene's own print choices. Set by
    /// [`Self::mark_project_state_unsaved`], OR'd into [`Self::is_dirty`], and
    /// cleared by [`Self::mark_saved`] and on install/clear. This tracks
    /// UNSAVED changes only (the signal that drives Save and the close-prompt);
    /// whether a computed slice still matches the geometry and settings it was
    /// produced from is a separate concern.
    unsaved_project_state: Cell<bool>,
    /// The slice produced for the current scene, held for the session and
    /// consulted by the re-slice decision. Cleared when a scene is installed or
    /// cleared: fresh geometry means the old slices no longer describe it.
    slice_result: RefCell<Option<SliceResult>>,
    /// Set when the undo-stack cap evicts an applied edit since the held slice
    /// result was recorded. Mirrors [`Self::evicted_since_save`] for the slice
    /// point, so the empty-stack comparison in [`Self::slice_geometry_changed`]
    /// cannot under-report a change: an evicted edit stays applied with no way
    /// to undo it.
    evicted_since_slice: Cell<bool>,
    /// Bumped every time a new slice result is stored. The slice-view cap keys
    /// its cached texture on this plus the layer index, so a re-slice
    /// invalidates the cap.
    slice_generation: Cell<u64>,
    /// Bumped by every install and clear, so a save prepared against a scene
    /// can tell it is completing against another.
    scene_generation: Cell<u64>,
    /// Bumped by every project-level change, giving those the same comparable
    /// identity committed edits get from their ids.
    project_state_generation: Cell<u64>,
}

/// The edit that swaps the mesh a leaf shows for `after`, whichever way the
/// leaf holds it: inline (an imported model) or through a mesh resource (a
/// pasted fragment, such as the support mesh). `None` when `leaf` holds no
/// mesh. A shared resource changes for every leaf that uses it.
pub(crate) fn leaf_mesh_swap(
    scene: &Scene,
    leaf: NodeId,
    after: Arc<sc_mesh_core::IndexedMesh>,
) -> Option<SceneEdit> {
    match &scene.node(leaf)?.kind {
        NodeKind::Leaf(LeafPayload::Mesh(before)) => Some(SceneEdit::ReplaceLeafMesh {
            node_id: leaf,
            before: before.clone(),
            after,
        }),
        NodeKind::Leaf(LeafPayload::MeshResource(rid)) => {
            let rid = *rid;
            Some(SceneEdit::ReplaceMeshResource {
                rid,
                before: scene.mesh_resource_arc(rid)?.clone(),
                after,
            })
        }
        _ => None,
    }
}

impl SceneDocument {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            spatial: RefCell::new(None),
            source_path: RefCell::new(None),
            origin: Cell::new(DocumentOrigin::PlainMesh),
            print_selection: RefCell::new(PrintSelection::default()),
            embedded_profiles: RefCell::new(Vec::new()),
            selection: SelectionState::new(),
            listeners: RefCell::new(Vec::new()),
            next_listener_id: Cell::new(0),
            undo_stack: RefCell::new(VecDeque::new()),
            redo_stack: RefCell::new(VecDeque::new()),
            next_edit_id: Cell::new(1),
            save_point: Cell::new(None),
            evicted_since_save: Cell::new(false),
            unsaved_project_state: Cell::new(false),
            slice_result: RefCell::new(None),
            evicted_since_slice: Cell::new(false),
            slice_generation: Cell::new(0),
            scene_generation: Cell::new(0),
            project_state_generation: Cell::new(0),
        })
    }

    /// Wraps a [`Scene`] in a fresh (lazy) [`SceneSpatial`] and installs it.
    #[cfg(test)]
    pub(crate) fn install_graph(&self, graph: Scene, source_path: PathBuf) {
        self.install_spatial(SceneSpatial::from_graph(graph), source_path, None);
    }

    /// Installs a scene opened from a project file: like
    /// [`Self::install_graph`] but with the project data the file carried.
    #[cfg(test)]
    pub(crate) fn install_project_graph(
        &self,
        graph: Scene,
        source_path: PathBuf,
        payload: ProjectPayload,
    ) {
        self.install_spatial(SceneSpatial::from_graph(graph), source_path, Some(payload));
    }

    /// Installs a [`SceneSpatial`] (typically pre-primed by the load worker
    /// thread). Resets selection and clears history just like
    /// [`Self::install_graph`]. `project_payload` is the project data the
    /// source file carried, `None` for plain mesh files.
    pub(crate) fn install_spatial(
        &self,
        spatial: SceneSpatial,
        source_path: PathBuf,
        project_payload: Option<ProjectPayload>,
    ) {
        let was_dirty = self.is_dirty();
        self.bump_scene_generation();
        *self.spatial.borrow_mut() = Some(spatial);
        *self.source_path.borrow_mut() = Some(source_path);
        let (selection, embedded) = match project_payload {
            Some(payload) => {
                self.origin.set(DocumentOrigin::Project);
                split_payload(payload)
            }
            None => {
                self.origin.set(DocumentOrigin::PlainMesh);
                (PrintSelection::default(), Vec::new())
            }
        };
        *self.print_selection.borrow_mut() = selection;
        *self.embedded_profiles.borrow_mut() = embedded;
        self.selection.set(Selection::None);
        // Object indices in the new scene have no relationship to the outgoing
        // one's, so any pending edits would target the wrong objects on replay.
        let stacks_changed = self.clear_history();
        // A freshly installed scene matches its source by definition.
        self.save_point.set(None);
        self.evicted_since_save.set(false);
        self.unsaved_project_state.set(false);
        // A new scene invalidates any slice produced for the old one.
        *self.slice_result.borrow_mut() = None;
        self.evicted_since_slice.set(false);
        self.notify(SceneEvent::Installed);
        if stacks_changed {
            self.notify(SceneEvent::UndoStackChanged);
        }
        self.notify_dirty_change(was_dirty);
    }

    pub(crate) fn clear_scene(&self) {
        let was_dirty = self.is_dirty();
        self.bump_scene_generation();
        *self.spatial.borrow_mut() = None;
        *self.source_path.borrow_mut() = None;
        self.origin.set(DocumentOrigin::PlainMesh);
        *self.print_selection.borrow_mut() = PrintSelection::default();
        self.embedded_profiles.borrow_mut().clear();
        self.selection.set(Selection::None);
        let stacks_changed = self.clear_history();
        self.save_point.set(None);
        self.evicted_since_save.set(false);
        self.unsaved_project_state.set(false);
        *self.slice_result.borrow_mut() = None;
        self.evicted_since_slice.set(false);
        self.notify(SceneEvent::Cleared);
        if stacks_changed {
            self.notify(SceneEvent::UndoStackChanged);
        }
        self.notify_dirty_change(was_dirty);
    }

    /// Reports whether the scene or its project-level state differs from the
    /// state it was loaded or last saved in (the save-dirty / unsaved-changes
    /// signal). Scene contents are judged by committed edits only (a
    /// mid-gesture mutation that has not been pushed yet does not count);
    /// project-level state such as the print choices contributes through
    /// [`Self::mark_project_state_unsaved`]. `false` whenever no scene is
    /// loaded.
    ///
    /// Clean means the undo stack's top entry is the one recorded by
    /// [`Self::mark_saved`]. A top entry matching the save point proves the
    /// content matches the saved state regardless of eviction: entry ids are
    /// unique, and eviction only forgets undo ability without mutating the
    /// scene. The empty-vs-empty match is the only exception; it is only
    /// trusted while no eviction has happened since the save point (see
    /// [`Self::evicted_since_save`]). The comparison can over-report dirty (the
    /// save point may sit deeper in the stack or be evicted), never
    /// under-report it.
    pub(crate) fn is_dirty(&self) -> bool {
        if !self.is_loaded() {
            return false;
        }
        if self.unsaved_project_state.get() {
            return true;
        }
        let top = self.top_edit_id();
        if top != self.save_point.get() {
            return true;
        }
        top.is_none() && self.evicted_since_save.get()
    }

    /// Records the current history position as the saved state, so
    /// [`Self::is_dirty`] reads `false` until the next committed change.
    pub(crate) fn mark_saved(&self) {
        let was_dirty = self.is_dirty();
        self.save_point.set(self.top_edit_id());
        self.evicted_since_save.set(false);
        self.unsaved_project_state.set(false);
        self.notify_dirty_change(was_dirty);
    }

    /// Composes the project payload for a save: the scene's own choices, with a
    /// missing printer or resin filled from `defaults`, and the held embedded
    /// profile documents.
    pub(crate) fn compose_project_payload(&self, defaults: &PrintDefaults) -> ProjectPayload {
        let PrintSelection {
            printer,
            resin,
            first_layer_height,
            output_format,
        } = self
            .print_selection
            .borrow()
            .in_force(defaults)
            .recorded_in_project();
        ProjectPayload {
            printer,
            resin,
            first_layer_height,
            output_format,
            embedded: self.embedded_profiles.borrow().clone(),
        }
    }

    /// Reports whether this document is a project: it was opened from a project
    /// file or has been saved as one.
    pub(crate) fn is_project(&self) -> bool {
        self.is_loaded() && self.origin.get() == DocumentOrigin::Project
    }

    /// Captures the document state a save is being prepared from, to be handed
    /// back to [`Self::record_project_save_for`] on completion.
    pub(crate) fn save_stamp(&self) -> SaveStamp {
        SaveStamp {
            scene: self.scene_generation.get(),
            top_edit: self.top_edit_id(),
            project_state: self.project_state_generation.get(),
        }
    }

    /// Records that the document was written as a project to `target`, against
    /// the state the write was prepared from.
    ///
    /// Only a document still matching `prepared` is marked saved: the file
    /// holds that state and nothing later. A document that moved on keeps its
    /// dirty marker, and one that now holds a different scene is left
    /// untouched, since the file describes neither its contents nor its
    /// identity.
    pub(crate) fn record_project_save_for(
        &self,
        payload: ProjectPayload,
        target: PathBuf,
        prepared: SaveStamp,
    ) -> SaveRecorded {
        let current = self.save_stamp();
        if current.scene != prepared.scene {
            return SaveRecorded::Replaced;
        }
        // The file came from this document, so it is what Save overwrites from
        // here on regardless of what else moved.
        *self.source_path.borrow_mut() = Some(target);
        self.origin.set(DocumentOrigin::Project);
        if current != prepared {
            return SaveRecorded::Drifted;
        }
        let (selection, embedded) = split_payload(payload);
        *self.print_selection.borrow_mut() = selection;
        *self.embedded_profiles.borrow_mut() = embedded;
        self.mark_saved();
        SaveRecorded::Clean
    }

    /// Returns a copy of the scene's own print choices.
    pub(crate) fn print_selection(&self) -> PrintSelection {
        self.print_selection.borrow().clone()
    }

    /// Sets the scene's own printer-file choice. Marks the document save-dirty
    /// only when the value actually changes. Returns whether it changed.
    pub(crate) fn set_selected_output_format(
        &self,
        output_format: Option<OutputFormatBinding>,
    ) -> bool {
        self.set_selection_field(|selection| &mut selection.output_format, output_format)
    }

    /// Sets the scene's own printer choice. Marks the document save-dirty only
    /// when the value actually changes. Returns whether it changed.
    pub(crate) fn set_selected_printer(&self, printer: Option<PrinterId>) -> bool {
        self.set_selection_field(|selection| &mut selection.printer, printer)
    }

    /// Sets the scene's own resin choice. Marks the document save-dirty only
    /// when the value actually changes. Returns whether it changed.
    pub(crate) fn set_selected_resin(&self, resin: Option<MaterialId>) -> bool {
        self.set_selection_field(|selection| &mut selection.resin, resin)
    }

    /// Sets the scene's own first-layer height. Marks the document save-dirty
    /// only when the value actually changes. Returns whether it changed.
    pub(crate) fn set_selected_first_layer_height(&self, height: Option<PositiveFiniteMm>) -> bool {
        self.set_selection_field(|selection| &mut selection.first_layer_height, height)
    }

    /// Writes a field of the print selection, marking the document save-dirty
    /// only when the value actually changes. Returns whether it changed.
    fn set_selection_field<T: PartialEq>(
        &self,
        field: impl FnOnce(&mut PrintSelection) -> &mut Option<T>,
        value: Option<T>,
    ) -> bool {
        let changed = {
            let mut selection = self.print_selection.borrow_mut();
            let slot = field(&mut selection);
            if *slot == value {
                false
            } else {
                *slot = value;
                true
            }
        };
        if changed {
            self.mark_project_state_unsaved();
        }
        changed
    }

    /// Captures the installed scene's identity together with its committed-edit
    /// head, so a later comparison can tell whether the geometry moved on.
    pub(crate) fn scene_revision(&self) -> SceneRevision {
        SceneRevision {
            scene: self.scene_generation.get(),
            top_edit: self.top_edit_id(),
        }
    }

    /// Reports whether the committed geometry changed since `since` was taken.
    /// Installing or clearing a scene in between always counts as a change: the
    /// geometry `since` describes is gone. Within a scene this mirrors
    /// [`Self::is_dirty`]'s comparison: a differing top edit id means changed;
    /// a matching empty stack (`None`) is trusted only while no applied edit
    /// has been evicted since the slice was recorded. Can over-report a change,
    /// never under-report it.
    pub(crate) fn slice_geometry_changed(&self, since: SceneRevision) -> bool {
        let current = self.scene_revision();
        if current != since {
            return true;
        }
        current.top_edit.is_none() && self.evicted_since_slice.get()
    }

    /// Stores the slice produced for the current scene, marking the present
    /// geometry as its baseline (a fresh eviction window). Slice staleness is
    /// tracked separately from the save-dirty signal, so this does not touch
    /// [`Self::is_dirty`].
    pub(crate) fn set_slice_result(&self, result: SliceResult) {
        *self.slice_result.borrow_mut() = Some(result);
        self.evicted_since_slice.set(false);
        self.slice_generation
            .set(self.slice_generation.get().wrapping_add(1));
        self.notify(SceneEvent::SliceChanged);
    }

    /// Returns the held slice, or `None` when the current scene has not been
    /// sliced. Clones the contours; [`Self::with_slice_layer`] reads a layer in
    /// place.
    pub(crate) fn held_slice(&self) -> Option<SliceResult> {
        self.slice_result.borrow().clone()
    }

    /// Returns the parameters the held slice ran with, or `None` when the
    /// current scene has not been sliced.
    pub(crate) fn slice_params(&self) -> Option<SliceParams> {
        self.slice_result
            .borrow()
            .as_ref()
            .map(|result| result.params.clone())
    }

    /// Returns how many layers the held slice has, or `None` when no slice is
    /// held.
    pub(crate) fn slice_layer_count(&self) -> Option<u32> {
        self.slice_result
            .borrow()
            .as_ref()
            .map(|result| result.contours.schedule().total_layers())
    }

    /// Returns the top Z of the held slice's layer at `index`, in mm above the
    /// plate, or `None` when no slice is held or `index` is past the last
    /// layer.
    pub(crate) fn slice_layer_top_z(&self, index: u32) -> Option<f64> {
        self.slice_result.borrow().as_ref().and_then(|result| {
            result
                .contours
                .schedule()
                .iter_slab_tops()
                .nth(index as usize)
        })
    }

    /// Returns a counter that changes every time a slice result is stored, so
    /// callers caching per-slice data can tell a re-slice happened.
    pub(crate) fn slice_generation(&self) -> u64 {
        self.slice_generation.get()
    }

    /// Runs `f` against the held slice's layer at `index`, or returns `None`
    /// when no slice is held or the index is past the last layer. The layer is
    /// borrowed in place, so the cap raster is built without cloning the
    /// contour.
    pub(crate) fn with_slice_layer<R>(
        &self,
        index: u32,
        f: impl FnOnce(&ContourLayer) -> R,
    ) -> Option<R> {
        self.slice_result
            .borrow()
            .as_ref()
            .and_then(|result| result.contours.stack().layers().get(index as usize).map(f))
    }

    /// Packages the held slices for persistence, or returns `None` when writing
    /// them would be wrong (no slice is held, the geometry changed since the
    /// slice ran, or the slice ran under a schedule that no longer matches
    /// `current_schedule`). Printer, resin and anti-aliasing choices do not
    /// count here, because the vector slices do not depend on them.
    pub(crate) fn slices_for_persist(
        &self,
        current_schedule: &LayerSchedule,
    ) -> Option<(PlateSlices, String)> {
        let held = self.slice_result.borrow();
        let held = held.as_ref()?;
        if self.slice_geometry_changed(held.params.geometry) {
            return None;
        }
        let schedule = held.contours.schedule();
        if schedule != current_schedule {
            return None;
        }
        let plate = PlateSlices {
            contours: held.contours.stack().clone(),
            slab_tops: schedule.iter_slab_tops().collect(),
        };
        Some((plate, held.token.clone()))
    }

    fn top_edit_id(&self) -> Option<u64> {
        self.undo_stack.borrow().back().map(|entry| entry.id)
    }

    /// Fires [`SceneEvent::DirtyChanged`] when the dirty value no longer
    /// matches the `was_dirty` snapshot the caller took before mutating.
    fn notify_dirty_change(&self, was_dirty: bool) {
        if self.is_dirty() != was_dirty {
            self.notify(SceneEvent::DirtyChanged);
        }
    }

    /// Records an unsaved change to project-level state that does not live on
    /// the undo stack (the scene's own print choices). Feeds [`Self::is_dirty`]
    /// (the save-dirty / unsaved-changes signal that drives Save and the
    /// close-prompt) and is cleared by [`Self::mark_saved`]. Slice staleness,
    /// whether a computed slice still matches its inputs, is tracked
    /// separately. Fires [`SceneEvent::DirtyChanged`] on an actual clean->dirty
    /// flip.
    fn mark_project_state_unsaved(&self) {
        let was_dirty = self.is_dirty();
        self.unsaved_project_state.set(true);
        self.project_state_generation
            .set(self.project_state_generation.get().wrapping_add(1));
        self.notify_dirty_change(was_dirty);
    }

    fn bump_scene_generation(&self) {
        self.scene_generation
            .set(self.scene_generation.get().wrapping_add(1));
    }

    /// Drops any pending undo/redo entries. Returns whether either stack was
    /// non-empty before the call: callers that fire `UndoStackChanged` only
    /// when an observable change happened use this to gate the notification.
    fn clear_history(&self) -> bool {
        let mut undo = self.undo_stack.borrow_mut();
        let mut redo = self.redo_stack.borrow_mut();
        let had = !undo.is_empty() || !redo.is_empty();
        undo.clear();
        redo.clear();
        had
    }

    pub(crate) fn selection(&self) -> Selection {
        self.selection.current()
    }

    pub(crate) fn set_selection(&self, sel: Selection) {
        if self.selection.current() == sel {
            return;
        }
        self.selection.set(sel);
        self.notify(SceneEvent::SelectionChanged);
    }

    /// Returns the currently-selected leaf's stable [`NodeId`], or `None` when
    /// no selection exists. Equivalent to
    /// `self.selection().primary().map(LeafId::node_id)` but cheaper to call
    /// repeatedly.
    pub(crate) fn selection_primary_node_id(&self) -> Option<NodeId> {
        self.selection.primary_node_id()
    }

    /// Applies a typed scene edit. The single mutation choke-point on
    /// `SceneDocument`: routes through [`SceneSpatial::apply_edit`] (which
    /// keeps spatial caches in sync) and fires [`SceneEvent::Mutated`] or
    /// [`SceneEvent::Restructured`].
    ///
    /// On [`EditNotApplied`] the scene, the caches, and the selection are
    /// untouched and no event fires.
    ///
    /// Does **not** touch the undo stack: `push_edit` is a separate concern so
    /// live-drag ticks can mutate the scene without flooding the history.
    /// Callers that mean "apply and record" invoke `apply_edit` followed by
    /// `push_edit` at gesture-end.
    pub(crate) fn apply_edit(&self, edit: SceneEdit) -> Result<(), EditNotApplied> {
        let mut slot = self.spatial.borrow_mut();
        let Some(spatial) = slot.as_mut() else {
            return Err(EditNotApplied::NoSceneLoaded);
        };
        // Decide selection-clearing and the post-mutation event kind BEFORE
        // moving the edit into spatial.apply_edit: the typed-edit enum is not
        // `Copy`, and `RemoveLeaf` may be nested under a `Compound` produced by
        // paste-undo, so a plain `match` post-apply is not enough.
        let drops_selection = edit_removes_selected_leaf(&edit, &self.selection);
        let event = if edit.is_structural() {
            SceneEvent::Restructured
        } else {
            SceneEvent::Mutated
        };
        spatial.apply_edit(edit).map_err(EditNotApplied::Rejected)?;
        drop(slot);
        // If the edit tombstoned the currently-selected leaf, drop the
        // now-dangling selection so observers don't keep pointing at an empty
        // arena slot. Fires SelectionChanged after the mutation event so
        // listeners that recompute on mutation see the updated selection.
        let selection_cleared = if drops_selection {
            self.selection.set(Selection::None);
            true
        } else {
            false
        };
        self.notify(event);
        if selection_cleared {
            self.notify(SceneEvent::SelectionChanged);
        }
        Ok(())
    }

    /// Records a reversible edit on the undo stack. No-op edits (`before ==
    /// after`) are dropped silently so a gesture that began and ended in the
    /// same place doesn't clutter history. A successful push clears the redo
    /// stack: a fresh edit invalidates the previously-rolled-back trail.
    pub(crate) fn push_edit(&self, edit: SceneEdit) {
        if edit.is_noop() {
            return;
        }
        let was_dirty = self.is_dirty();
        let id = self.next_edit_id.get();
        self.next_edit_id.set(id + 1);
        {
            let mut undo = self.undo_stack.borrow_mut();
            if undo.len() == UNDO_STACK_CAP {
                undo.pop_front();
                self.evicted_since_save.set(true);
                self.evicted_since_slice.set(true);
            }
            undo.push_back(HistoryEntry { id, edit });
        }
        self.redo_stack.borrow_mut().clear();
        self.notify(SceneEvent::UndoStackChanged);
        self.notify_dirty_change(was_dirty);
    }

    /// Allocates a fresh [`NodeId`] and installs a mesh-resource-backed leaf at
    /// it as the last scene root: `allocate`, `AddLeaf` and `push_edit` in 1
    /// call. Returns the new id (`Ok(None)` when no scene is loaded, `Err` when
    /// the scene rejected the edit; either way nothing is recorded).
    ///
    /// The corresponding edit goes onto the undo stack: a subsequent `undo`
    /// pops it as a `RemoveLeaf` and tombstones the leaf.
    #[allow(dead_code)]
    pub(crate) fn add_root_leaf(
        &self,
        mesh: MeshResourceId,
        local_transform: Transform3D,
    ) -> Result<Option<NodeId>, SceneError> {
        let (node_id, position) = {
            let mut slot = self.spatial.borrow_mut();
            let Some(spatial) = slot.as_mut() else {
                return Ok(None);
            };
            let position = spatial.graph().roots().len();
            (spatial.allocate_unbound_node_id(), position)
        };
        let edit = SceneEdit::AddLeaf {
            node_id,
            attachment: LeafAttachment::Root { position },
            payload: LeafPayload::MeshResource(mesh),
            local_transform,
            meta: None,
        };
        match self.apply_edit(edit.clone()) {
            Ok(()) => {
                self.push_edit(edit);
                Ok(Some(node_id))
            }
            Err(EditNotApplied::NoSceneLoaded) => Ok(None),
            Err(EditNotApplied::Rejected(error)) => Err(error),
        }
    }

    /// Deletes the currently-selected leaf, recording the operation as 1 undo
    /// entry.
    ///
    /// Works on any leaf the loaders or paste flow can produce:
    /// - inline-mesh and mesh-resource payloads alike;
    /// - root leaves (STL-loaded, paste-installed) and parented leaves
    ///   (OBJ-loaded under their identity-Group root) alike. The leaf's parent
    ///   and position are recorded in the edit so undo restores sibling order.
    ///
    /// Bails with `Ok(false)` (no scene mutation, no undo push) when no scene
    /// is loaded, the selection is `Selection::None`, or the selected node is
    /// not a leaf; errs, recording nothing, when the scene rejected the edit.
    ///
    /// When a mesh-resource leaf was the last live reference to its
    /// `MeshResourceId`, the recorded edit is a `Compound` that also tombstones
    /// the resource so the arena stays in sync with the live-leaf population.
    /// Inline-mesh leaves carry their own `Arc`, so removal of an inline-mesh
    /// leaf never produces a Compound.
    ///
    /// Returns `Ok(true)` on a successful deletion. Selection clearing and the
    /// `Restructured` event are handled by [`Self::apply_edit`].
    pub(crate) fn delete_selected_leaf(&self) -> Result<bool, SceneError> {
        let Some(node_id) = self.selection_primary_node_id() else {
            return Ok(false);
        };
        let edit = self
            .with_scene(|spatial| SceneEdit::remove_leaf_with_gc(spatial.graph(), node_id))
            .flatten();
        let Some(edit) = edit else {
            return Ok(false);
        };
        match self.apply_edit(edit.clone()) {
            Ok(()) => {
                self.push_edit(edit);
                Ok(true)
            }
            Err(EditNotApplied::NoSceneLoaded) => Ok(false),
            Err(EditNotApplied::Rejected(error)) => Err(error),
        }
    }

    /// Pastes a [`SceneFragment`] into the current scene as a new root.
    /// `placement_translation` overrides the fragment root's translation
    /// component (the caller's mouse-projection / placement-search result);
    /// rotation + scale carried by the fragment root are preserved.
    ///
    /// Mesh resources in the fragment are deduped against the target scene by
    /// `Arc::as_ptr` identity: a resource that's already registered (same-scene
    /// paste, repeated paste after an AddMeshResource has landed) is reused
    /// without emitting a new edit; otherwise an `AddMeshResource` joins the
    /// compound. The whole paste reaches the undo stack as 1 entry so a single
    /// `Ctrl+Z` reverses the entire gesture.
    ///
    /// Returns the new root leaf's `NodeId`; `Ok(None)` when no scene is loaded
    /// or the fragment carries no mesh-resource leaf roots (the only shape the
    /// current builder produces); `Err`, with nothing recorded and the
    /// selection untouched, when the scene rejected the edit.
    pub(crate) fn paste_fragment(
        &self,
        fragment: &SceneFragment,
        placement_translation: [f64; 3],
    ) -> Result<Option<NodeId>, SceneError> {
        let Some((compound, new_root)) =
            self.paste_fragment_compound(fragment, placement_translation)
        else {
            return Ok(None);
        };
        match self.apply_edit(compound.clone()) {
            Ok(()) => {
                self.push_edit(compound);
                self.set_selection(Selection::One(crate::selection::LeafId(new_root)));
                Ok(Some(new_root))
            }
            Err(EditNotApplied::NoSceneLoaded) => Ok(None),
            Err(EditNotApplied::Rejected(error)) => Err(error),
        }
    }

    fn paste_fragment_compound(
        &self,
        fragment: &SceneFragment,
        placement_translation: [f64; 3],
    ) -> Option<(SceneEdit, NodeId)> {
        let mut slot = self.spatial.borrow_mut();
        let spatial = slot.as_mut()?;
        let frag_scene = fragment.scene();
        let mut rid_map: HashMap<MeshResourceId, MeshResourceId> = HashMap::new();
        let mut edits: Vec<SceneEdit> = Vec::new();

        // Step 1: dedupe fragment's mesh resources against the target scene by
        // Arc pointer identity. Snapshot the rid list first so the immutable
        // mesh_resources_iter borrow on `frag_scene` doesn't fight subsequent
        // mutable spatial calls.
        let frag_rids: Vec<MeshResourceId> = frag_scene
            .mesh_resources_iter()
            .map(|(rid, _)| rid)
            .collect();
        for frag_rid in frag_rids {
            let frag_arc = frag_scene
                .mesh_resource_arc(frag_rid)
                .expect("frag_scene yielded a live rid");
            let target_rid = match spatial.graph().find_mesh_resource_by_arc(frag_arc) {
                Some(existing) => existing,
                None => {
                    let new_rid = spatial.allocate_unbound_mesh_resource_id();
                    edits.push(SceneEdit::AddMeshResource {
                        rid: new_rid,
                        mesh: Arc::clone(frag_arc),
                    });
                    new_rid
                }
            };
            rid_map.insert(frag_rid, target_rid);
        }

        // Step 2: walk the fragment's roots, allocate target node ids, emit
        // AddLeaf for each mesh-resource leaf root.
        let mut first_new_root: Option<NodeId> = None;
        // Pasted leaves land after the roots already there, one after another,
        // and none of the edits before them adds a root.
        let mut position = spatial.graph().roots().len();
        for &frag_root in frag_scene.roots() {
            let frag_node = frag_scene
                .node(frag_root)
                .expect("frag_scene.roots() yielded a live id");
            let frag_rid = match &frag_node.kind {
                NodeKind::Leaf(LeafPayload::MeshResource(rid)) => *rid,
                NodeKind::Leaf(LeafPayload::Mesh(_))
                | NodeKind::Group { .. }
                | NodeKind::Csg { .. }
                | NodeKind::Reference { .. } => {
                    // The SceneFragment builder only emits mesh-resource leaf
                    // roots; other shapes are skipped.
                    continue;
                }
            };
            let target_rid = rid_map[&frag_rid];
            let target_node_id = spatial.allocate_unbound_node_id();
            if first_new_root.is_none() {
                first_new_root = Some(target_node_id);
            }
            let final_xform =
                substitute_translation(frag_node.local_transform, placement_translation);
            edits.push(SceneEdit::AddLeaf {
                node_id: target_node_id,
                attachment: LeafAttachment::Root { position },
                payload: LeafPayload::MeshResource(target_rid),
                local_transform: final_xform,
                meta: None,
            });
            position += 1;
        }

        let new_root = first_new_root?;
        let compound = if edits.len() == 1 {
            edits.into_iter().next().expect("len == 1")
        } else {
            SceneEdit::Compound(edits)
        };
        Some((compound, new_root))
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo_stack.borrow().is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo_stack.borrow().is_empty()
    }

    /// Pops the most recent edit, applies its inverse, and moves the entry onto
    /// the redo stack. Returns `Ok(true)` if an edit was applied, `Ok(false)`
    /// if there was nothing to undo (empty stack or no scene loaded), and the
    /// scene's rejection otherwise; on anything but `Ok(true)` the stacks are
    /// left as they were.
    pub(crate) fn undo(&self) -> Result<bool, SceneError> {
        let was_dirty = self.is_dirty();
        let Some(entry) = self.undo_stack.borrow_mut().pop_back() else {
            return Ok(false);
        };
        match self.apply_edit(entry.edit.inverse()) {
            Ok(()) => {}
            Err(reason) => {
                // Restore the popped entry so the stack stays consistent with
                // the user's history.
                self.undo_stack.borrow_mut().push_back(entry);
                return match reason {
                    EditNotApplied::NoSceneLoaded => Ok(false),
                    EditNotApplied::Rejected(error) => Err(error),
                };
            }
        }
        let mut redo = self.redo_stack.borrow_mut();
        if redo.len() == UNDO_STACK_CAP {
            redo.pop_front();
        }
        redo.push_back(entry);
        drop(redo);
        self.notify(SceneEvent::UndoStackChanged);
        self.notify_dirty_change(was_dirty);
        Ok(true)
    }

    /// Pops the most recent rolled-back edit, applies it forward, and moves the
    /// entry back onto the undo stack. Returns `Ok(true)` if an edit was
    /// applied, `Ok(false)` if there was nothing to redo (empty stack or no
    /// scene loaded), and the scene's rejection otherwise; on anything but
    /// `Ok(true)` the stacks are left as they were.
    pub(crate) fn redo(&self) -> Result<bool, SceneError> {
        let was_dirty = self.is_dirty();
        let Some(entry) = self.redo_stack.borrow_mut().pop_back() else {
            return Ok(false);
        };
        match self.apply_edit(entry.edit.clone()) {
            Ok(()) => {}
            Err(reason) => {
                self.redo_stack.borrow_mut().push_back(entry);
                return match reason {
                    EditNotApplied::NoSceneLoaded => Ok(false),
                    EditNotApplied::Rejected(error) => Err(error),
                };
            }
        }
        let mut undo = self.undo_stack.borrow_mut();
        if undo.len() == UNDO_STACK_CAP {
            undo.pop_front();
            self.evicted_since_save.set(true);
            self.evicted_since_slice.set(true);
        }
        undo.push_back(entry);
        drop(undo);
        self.notify(SceneEvent::UndoStackChanged);
        self.notify_dirty_change(was_dirty);
        Ok(true)
    }

    /// Registers a listener. Returns a handle whose `Drop` removes the
    /// listener. Listeners fire in registration order.
    pub(crate) fn subscribe<F>(self: &Rc<Self>, listener: F) -> SceneSubscription
    where
        F: Fn(SceneEvent) + 'static,
    {
        let id = self.next_listener_id.get();
        self.next_listener_id.set(id.wrapping_add(1));
        self.listeners
            .borrow_mut()
            .push((id, Rc::new(listener) as Listener));
        SceneSubscription {
            id,
            document: Rc::downgrade(self),
        }
    }

    fn notify(&self, event: SceneEvent) {
        // Snapshot listeners so a listener body can re-enter `subscribe` /
        // `unsubscribe` without re-borrowing the cell.
        let snapshot: Vec<Listener> = self
            .listeners
            .borrow()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in snapshot {
            listener(event);
        }
    }

    /// Returns the path the current scene was loaded from. `None` when no scene
    /// is loaded; cloned out so the caller doesn't extend the borrow.
    #[cfg(test)]
    pub(crate) fn source_path(&self) -> Option<PathBuf> {
        self.source_path.borrow().clone()
    }

    /// Runs `f` on the borrowed source path, for callers that need the path
    /// only inside a closure (avoids the clone in `source_path`).
    pub(crate) fn with_source_path<R>(&self, f: impl FnOnce(&Path) -> R) -> Option<R> {
        self.source_path.borrow().as_deref().map(f)
    }

    /// Returns the source file's name with extension (for example
    /// `widget.stl`), used as the window title. `None` when no scene is loaded
    /// or the path has no final component.
    pub(crate) fn source_file_name(&self) -> Option<String> {
        self.with_source_path(|p| {
            p.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_owned())
        })
        .flatten()
    }

    /// Runs `f` against the loaded [`SceneSpatial`], if any. Returns `None`
    /// when no scene is installed. Callers must keep the closure short: the
    /// internal borrow is held for its duration. Use `spatial.graph()` for
    /// read-only navigation and `spatial.leaf(id)` for per-leaf queries.
    pub(crate) fn with_scene<R>(&self, f: impl FnOnce(&SceneSpatial) -> R) -> Option<R> {
        self.spatial.borrow().as_ref().map(f)
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.spatial.borrow().is_some()
    }
}

/// Returns `xform` with its translation component replaced by `translation`,
/// preserving the linear (rotation + scale) part.
fn substitute_translation(xform: Transform3D, translation: [f64; 3]) -> Transform3D {
    match xform {
        Transform3D::Identity => {
            Transform3D::Affine(AffineTransform3D::from_translation(translation))
        }
        Transform3D::Affine(a) => Transform3D::Affine(
            AffineTransform3D::from_linear_and_translation(*a.linear(), translation),
        ),
    }
}

/// Why [`SceneDocument::apply_edit`] did not apply an edit. Either way the
/// scene, the caches, and the selection are untouched.
#[derive(Debug)]
pub(crate) enum EditNotApplied {
    /// No document is loaded: a legal UI state (a shortcut firing with nothing
    /// open) that needs no dialog.
    NoSceneLoaded,
    /// The scene rejected the edit's preconditions. The GUI builds edits
    /// against the live scene, so this indicates a programming bug; callers
    /// surface it and record nothing.
    Rejected(SceneError),
}

/// Reports whether applying `edit` would tombstone the leaf the document's
/// `selection` currently points at. Handles the nested case where a
/// `RemoveLeaf` is inside a `Compound` (the shape paste-undo produces).
fn edit_removes_selected_leaf(edit: &SceneEdit, selection: &SelectionState) -> bool {
    let Some(selected) = selection.primary_node_id() else {
        return false;
    };
    fn touches(edit: &SceneEdit, selected: NodeId) -> bool {
        match edit {
            SceneEdit::RemoveLeaf { node_id, .. } => *node_id == selected,
            SceneEdit::Compound(edits) => edits.iter().any(|e| touches(e, selected)),
            SceneEdit::SetLocalTransform { .. }
            | SceneEdit::AddLeaf { .. }
            | SceneEdit::AddMeshResource { .. }
            | SceneEdit::RemoveMeshResource { .. }
            | SceneEdit::ReplaceMeshResource { .. }
            | SceneEdit::ReplaceLeafMesh { .. } => false,
        }
    }
    touches(edit, selected)
}

/// RAII handle to a `SceneDocument` subscription. Drop to unsubscribe.
pub(crate) struct SceneSubscription {
    id: u64,
    document: Weak<SceneDocument>,
}

impl Drop for SceneSubscription {
    fn drop(&mut self) {
        if let Some(doc) = self.document.upgrade() {
            doc.listeners.borrow_mut().retain(|(id, _)| *id != self.id);
        }
    }
}

#[cfg(test)]
mod scene_document_tests {
    use std::sync::Arc;

    use super::*;
    use crate::test_support::scenes::graph_with_one_object;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::transform::Transform3D;
    use sc_scene::{LeafPayload, NodeKind};

    /// Builds a loader-faithful empty graph: a single identity-Group root with
    /// no children, the same shape an empty-file load produces.
    fn empty_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        g
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from("/tmp/scene_document_fixture.obj")
    }

    fn translation(dx: f64) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation([dx, 0.0, 0.0]))
    }

    fn obj_transform(doc: &SceneDocument, idx: usize) -> Transform3D {
        doc.with_scene(|spatial| {
            let id = spatial
                .leaf_id_at_index(idx)
                .expect("test fixture leaf exists");
            spatial
                .leaf(id)
                .expect("test fixture leaf is queryable")
                .local_transform()
        })
        .unwrap()
    }

    #[test]
    fn new_constructs_a_document() {
        let _doc: Rc<SceneDocument> = SceneDocument::new();
    }

    #[test]
    fn fresh_document_has_no_scene() {
        let doc = SceneDocument::new();
        assert!(!doc.is_loaded());
        assert!(doc.with_scene(|_| ()).is_none());
        assert!(doc.source_path().is_none());
    }

    #[test]
    fn install_graph_makes_it_observable() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        assert!(doc.is_loaded());
        // The empty scene flattens to a single identity-root Group with no
        // leaves under it.
        assert!(doc.with_scene(|s| s.leaf_id_at_index(0).is_none()).unwrap());
        assert_eq!(doc.source_path(), Some(fixture_path()));
    }

    #[test]
    fn clear_scene_removes_it() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        doc.clear_scene();
        assert!(!doc.is_loaded());
        assert!(doc.with_scene(|_| ()).is_none());
        assert!(doc.source_path().is_none());
    }

    #[test]
    fn install_graph_replaces_a_prior_scene() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), PathBuf::from("/tmp/a.obj"));
        doc.install_graph(empty_graph(), PathBuf::from("/tmp/b.obj"));
        assert!(doc.is_loaded());
        assert_eq!(doc.source_path(), Some(PathBuf::from("/tmp/b.obj")));
    }

    #[test]
    fn install_spatial_uses_the_provided_spatial() {
        let doc = SceneDocument::new();
        doc.install_spatial(
            SceneSpatial::primed_graph(empty_graph()),
            fixture_path(),
            None,
        );
        assert!(doc.is_loaded());
        assert_eq!(doc.source_path(), Some(fixture_path()));
    }

    #[test]
    fn with_source_path_runs_only_when_a_path_is_set() {
        let doc = SceneDocument::new();
        assert!(doc.with_source_path(|_| 0_usize).is_none());
        doc.install_graph(empty_graph(), fixture_path());
        let stem = doc.with_source_path(|p| p.file_stem().unwrap().to_owned());
        assert_eq!(stem.as_deref(), Some("scene_document_fixture".as_ref()));
    }

    #[test]
    fn source_file_name_is_the_basename_with_extension() {
        let doc = SceneDocument::new();
        assert_eq!(doc.source_file_name(), None);
        doc.install_graph(empty_graph(), fixture_path());
        assert_eq!(
            doc.source_file_name().as_deref(),
            Some("scene_document_fixture.obj")
        );
    }

    #[test]
    fn source_file_name_follows_a_save_as_relocation() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let stamp = doc.save_stamp();
        doc.record_project_save_for(
            ProjectPayload::default(),
            PathBuf::from("/tmp/project.3mf"),
            stamp,
        );
        assert_eq!(doc.source_file_name().as_deref(), Some("project.3mf"));
    }

    #[test]
    fn fresh_document_has_no_selection() {
        let doc = SceneDocument::new();
        assert_eq!(doc.selection(), Selection::None);
        assert_eq!(doc.selection_primary_node_id(), None);
    }

    /// Picks the [`LeafId`] of the document's first leaf, for tests that need a
    /// real `Selection::One(_)` on a 1-leaf fixture.
    fn first_leaf_id(doc: &SceneDocument) -> crate::selection::LeafId {
        crate::selection::LeafId(
            doc.with_scene(|spatial| spatial.leaf_id_at_index(0))
                .flatten()
                .expect("fixture has at least one leaf"),
        )
    }

    #[test]
    fn set_selection_round_trips() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let leaf = first_leaf_id(&doc);
        doc.set_selection(Selection::One(leaf));
        assert_eq!(doc.selection(), Selection::One(leaf));
        assert_eq!(doc.selection_primary_node_id(), Some(leaf.node_id()));
    }

    #[test]
    fn install_graph_resets_selection() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let leaf = first_leaf_id(&doc);
        doc.set_selection(Selection::One(leaf));
        // Loading another scene drops the prior selection: its NodeId belonged
        // to the previous scene's arena.
        doc.install_graph(graph_with_one_object(), fixture_path());
        assert_eq!(doc.selection(), Selection::None);
    }

    #[test]
    fn clear_scene_resets_selection() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let leaf = first_leaf_id(&doc);
        doc.set_selection(Selection::One(leaf));
        doc.clear_scene();
        assert_eq!(doc.selection(), Selection::None);
    }

    fn capture_events() -> (Rc<RefCell<Vec<SceneEvent>>>, impl Fn(SceneEvent) + 'static) {
        let log: Rc<RefCell<Vec<SceneEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let log_for_listener = log.clone();
        let listener = move |event: SceneEvent| log_for_listener.borrow_mut().push(event);
        (log, listener)
    }

    #[test]
    fn install_clear_round_trip_fires_installed_then_cleared() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.install_graph(empty_graph(), fixture_path());
        doc.clear_scene();

        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::Installed, SceneEvent::Cleared]
        );
    }

    #[test]
    fn set_selection_only_fires_on_actual_change() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let leaf = first_leaf_id(&doc);
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.set_selection(Selection::None); // no change from initial
        doc.set_selection(Selection::One(leaf)); // changes
        doc.set_selection(Selection::One(leaf)); // no change
        doc.set_selection(Selection::None); // changes back

        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::SelectionChanged, SceneEvent::SelectionChanged]
        );
    }

    #[test]
    fn apply_edit_errors_with_no_scene_loaded() {
        let doc = SceneDocument::new();
        // The edit's contents are irrelevant: apply_edit short-circuits before
        // touching the edit when no scene is loaded. A NodeId pulled from a
        // throwaway spatial is the cheapest way to produce a typed instance.
        let some_node_id = SceneSpatial::primed_graph(graph_with_one_object())
            .leaf_id_at_index(0)
            .unwrap();
        let result = doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id: some_node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(matches!(result, Err(EditNotApplied::NoSceneLoaded)));
    }

    #[test]
    fn apply_edit_rejection_fires_no_events_and_keeps_selection() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let leaf = first_leaf_id(&doc);
        doc.set_selection(Selection::One(leaf));
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);
        // An id whose index lies beyond the loaded arena: the graph mutator
        // rejects it with DanglingNodeId.
        let mut scratch = Scene::new();
        for _ in 0..16 {
            scratch.add_group(None, Transform3D::Identity);
        }
        let bogus = scratch.allocate_unbound_node_id();
        let result = doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id: bogus,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(matches!(
            result,
            Err(EditNotApplied::Rejected(SceneError::DanglingNodeId { .. }))
        ));
        // The scene did not change, so observers hear nothing and the selection
        // stays where it was.
        assert!(log.borrow().is_empty());
        assert_eq!(doc.selection(), Selection::One(leaf));
    }

    #[test]
    fn undo_rejection_restores_the_entry_and_surfaces_the_error() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        // A poisoned history entry: its inverse (AddLeaf) targets the occupied
        // slot of the live leaf, so undo is rejected.
        let rid = Scene::new().allocate_unbound_mesh_resource_id();
        doc.push_edit(SceneEdit::RemoveLeaf {
            node_id,
            attachment: LeafAttachment::Root { position: 0 },
            payload: LeafPayload::MeshResource(rid),
            local_transform: Transform3D::Identity,
            meta: None,
        });
        assert!(doc.can_undo());
        let err = doc.undo().unwrap_err();
        assert!(matches!(
            err,
            SceneError::NodeSlotOccupied { id } if id == node_id
        ));
        // The entry stays at the top of the undo stack and nothing moved onto
        // the redo stack.
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn apply_edit_fires_mutated_when_a_scene_is_loaded() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let applied = doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(applied.is_ok());
        // Installed (from install_graph), then Mutated (from apply_edit).
        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::Installed, SceneEvent::Mutated]
        );
    }

    #[test]
    fn apply_edit_writes_after_into_the_scene() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let after = translation(5.0);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        })
        .unwrap();
        assert_eq!(obj_transform(&doc, 0), after);
    }

    #[test]
    fn dropping_subscription_removes_the_listener() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        {
            let _sub = doc.subscribe(listener);
            doc.install_graph(empty_graph(), fixture_path());
        } // subscription drops here
        doc.clear_scene();

        assert_eq!(*log.borrow(), vec![SceneEvent::Installed]);
    }

    #[test]
    fn fresh_document_has_no_history() {
        let doc = SceneDocument::new();
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn push_edit_makes_undo_available() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn push_edit_skips_no_op() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        });
        assert!(!doc.can_undo());
    }

    #[test]
    fn undo_restores_before_and_enables_redo() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        // Simulate a gesture: write `after`, then record the edit.
        let after = translation(5.0);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        })
        .unwrap();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        });

        assert!(doc.undo().unwrap());
        assert_eq!(obj_transform(&doc, 0), Transform3D::Identity);
        assert!(!doc.can_undo());
        assert!(doc.can_redo());
    }

    #[test]
    fn replace_edit_bumps_the_revision_and_undo_restores_arc_identity() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let before = doc
            .with_scene(|s| Arc::clone(s.graph().leaf_mesh_arc(node_id).unwrap()))
            .unwrap();
        let after = Arc::new(IndexedMesh::new(None, Vec::new(), Vec::new()));
        let captured = doc.scene_revision();
        let edit = SceneEdit::ReplaceLeafMesh {
            node_id,
            before: Arc::clone(&before),
            after: Arc::clone(&after),
        };
        doc.apply_edit(edit.clone()).unwrap();
        doc.push_edit(edit);
        // The push moved the revision: a slice captured before this
        // mesh-content replacement must read as stale.
        assert!(doc.slice_geometry_changed(captured));
        assert!(
            doc.with_scene(|s| Arc::ptr_eq(s.graph().leaf_mesh_arc(node_id).unwrap(), &after))
                .unwrap()
        );
        assert!(doc.undo().unwrap());
        assert!(
            doc.with_scene(|s| Arc::ptr_eq(s.graph().leaf_mesh_arc(node_id).unwrap(), &before))
                .unwrap()
        );
    }

    #[test]
    fn redo_replays_after_and_re_enables_undo() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let after = translation(5.0);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        })
        .unwrap();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        });

        doc.undo().unwrap();
        assert!(doc.redo().unwrap());
        assert_eq!(obj_transform(&doc, 0), after);
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn fresh_push_clears_redo_trail() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let a = translation(1.0);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: a,
        })
        .unwrap();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: a,
        });
        doc.undo().unwrap();
        assert!(doc.can_redo());

        // A new branch of history invalidates the redo trail.
        let b = translation(2.0);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: b,
        })
        .unwrap();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: b,
        });
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn undo_and_redo_are_no_ops_when_stacks_are_empty() {
        let doc = SceneDocument::new();
        assert!(!doc.undo().unwrap());
        assert!(!doc.redo().unwrap());
        doc.install_graph(empty_graph(), fixture_path());
        assert!(!doc.undo().unwrap());
        assert!(!doc.redo().unwrap());
    }

    #[test]
    fn stack_caps_at_undo_stack_cap() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        // Push UNDO_STACK_CAP + 5 distinct edits; the oldest 5 should be
        // silently dropped by `pop_front`.
        for i in 0..(UNDO_STACK_CAP + 5) {
            let dx = (i + 1) as f64;
            doc.push_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: translation(dx - 1.0),
                after: translation(dx),
            });
        }
        assert_eq!(doc.undo_stack.borrow().len(), UNDO_STACK_CAP);
        // Front of the deque is the oldest surviving entry; with 5 entries
        // dropped it should start at i = 5 (after = 6.0).
        let front = doc
            .undo_stack
            .borrow()
            .front()
            .map(|entry| entry.edit.clone())
            .unwrap();
        let SceneEdit::SetLocalTransform { after, .. } = front else {
            panic!("front entry was not a SetLocalTransform: {front:?}");
        };
        assert_eq!(after, translation(6.0));
    }

    #[test]
    fn install_graph_clears_undo_history() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(doc.can_undo());
        doc.install_graph(graph_with_one_object(), fixture_path());
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn clear_scene_clears_undo_history() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        doc.clear_scene();
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn push_edit_fires_undo_stack_changed() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        // The first committed edit also flips the document dirty.
        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::UndoStackChanged, SceneEvent::DirtyChanged]
        );
    }

    #[test]
    fn undo_fires_mutated_then_undo_stack_changed() {
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let after = translation(1.0);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        })
        .unwrap();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after,
        });
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.undo().unwrap();
        // The undo also drains the never-saved single-entry stack, returning
        // the document to its freshly-installed content, so the dirty flag
        // flips back to clean.
        assert_eq!(
            *log.borrow(),
            vec![
                SceneEvent::Mutated,
                SceneEvent::UndoStackChanged,
                SceneEvent::DirtyChanged,
            ]
        );
    }

    #[test]
    fn install_graph_emits_undo_stack_changed_only_when_history_was_non_empty() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        // Empty -> empty: only `Installed`.
        doc.install_graph(graph_with_one_object(), fixture_path());
        assert_eq!(*log.borrow(), vec![SceneEvent::Installed]);

        // Push an edit, then install again: the second install must also report
        // the history clear.
        let node_id = first_leaf_id(&doc).node_id();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        log.borrow_mut().clear();
        doc.install_graph(graph_with_one_object(), fixture_path());
        // The replaced document was dirty; the fresh one is clean, so the
        // install also reports the dirty flip.
        assert_eq!(
            *log.borrow(),
            vec![
                SceneEvent::Installed,
                SceneEvent::UndoStackChanged,
                SceneEvent::DirtyChanged,
            ]
        );
    }

    /// Builds a loader-faithful single-resource graph: an identity-Group root
    /// plus 1 mesh resource pre-registered for paste tests to reference.
    fn graph_with_one_resource() -> (Scene, MeshResourceId) {
        let mut g = empty_graph();
        let rid = g.add_mesh_resource(IndexedMesh::new(None, Vec::new(), Vec::new()));
        (g, rid)
    }

    #[test]
    fn add_root_leaf_with_no_scene_loaded_returns_none() {
        // Build, install, then clear so we have a syntactically valid
        // MeshResourceId left over from a gone scene; the method must still
        // bail.
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        doc.clear_scene();
        assert!(
            doc.add_root_leaf(rid, Transform3D::Identity)
                .unwrap()
                .is_none()
        );
        assert!(!doc.can_undo());
    }

    #[test]
    fn add_root_leaf_installs_the_leaf_and_returns_its_id() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let xform = translation(5.0);
        let id = doc
            .add_root_leaf(rid, xform)
            .unwrap()
            .expect("scene is loaded");
        let observed = doc.with_scene(|s| {
            let node = s.graph().node(id).expect("new node lives");
            (
                node.local_transform,
                matches!(
                    node.kind,
                    NodeKind::Leaf(LeafPayload::MeshResource(r)) if r == rid
                ),
                s.graph().roots().contains(&id),
            )
        });
        assert_eq!(observed, Some((xform, true, true)));
    }

    #[test]
    fn add_root_leaf_records_an_undoable_edit() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        assert!(!doc.can_undo());
        let _ = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .expect("scene is loaded");
        assert!(doc.can_undo());
    }

    #[test]
    fn add_root_leaf_undo_round_trip_restores_scene() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let id = doc
            .add_root_leaf(rid, translation(3.0))
            .unwrap()
            .expect("scene is loaded");
        assert_eq!(doc.with_scene(|s| s.graph().node(id).is_some()), Some(true));
        doc.undo().unwrap();
        assert_eq!(
            doc.with_scene(|s| s.graph().node(id).is_some()),
            Some(false)
        );
        doc.redo().unwrap();
        let kind_ok = doc.with_scene(|s| {
            let node = s.graph().node(id).expect("redo restores the node");
            matches!(node.kind, NodeKind::Leaf(LeafPayload::MeshResource(r)) if r == rid)
        });
        assert_eq!(kind_ok, Some(true));
    }

    #[test]
    fn remove_leaf_clears_selection_when_pointing_at_removed_node() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let id = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        doc.set_selection(Selection::One(crate::selection::LeafId(id)));
        assert_eq!(doc.selection_primary_node_id(), Some(id));
        // Undo the AddLeaf so the inverse RemoveLeaf fires.
        doc.undo().unwrap();
        assert_eq!(
            doc.selection_primary_node_id(),
            None,
            "undo of paste must drop a selection pointing at the gone leaf"
        );
    }

    #[test]
    fn remove_leaf_does_not_clear_selection_pointing_at_other_node() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        // Add 2 roots so removing one leaves the other selectable.
        let kept = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        let to_remove = doc.add_root_leaf(rid, translation(5.0)).unwrap().unwrap();
        doc.set_selection(Selection::One(crate::selection::LeafId(kept)));
        // Apply the RemoveLeaf directly (skip the undo stack so the
        // selection-clear logic is exercised on a non-matching id).
        let edit = SceneEdit::RemoveLeaf {
            node_id: to_remove,
            attachment: LeafAttachment::Root { position: 2 },
            payload: LeafPayload::MeshResource(rid),
            local_transform: translation(5.0),
            meta: None,
        };
        assert!(doc.apply_edit(edit).is_ok());
        assert_eq!(doc.selection_primary_node_id(), Some(kept));
    }

    #[test]
    fn remove_leaf_fires_selection_changed_when_selection_dropped() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let id = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        doc.set_selection(Selection::One(crate::selection::LeafId(id)));
        let events: Rc<RefCell<Vec<SceneEvent>>> = Rc::default();
        let log = events.clone();
        let _sub = doc.subscribe(move |ev| log.borrow_mut().push(ev));
        doc.undo().unwrap();
        assert_eq!(
            *events.borrow(),
            vec![
                SceneEvent::Restructured,
                SceneEvent::SelectionChanged,
                SceneEvent::UndoStackChanged,
                // Undoing the only committed edit returns the document to
                // clean.
                SceneEvent::DirtyChanged,
            ]
        );
    }

    #[test]
    fn delete_selected_leaf_with_no_scene_loaded_returns_false() {
        let doc = SceneDocument::new();
        assert!(!doc.delete_selected_leaf().unwrap());
        assert!(!doc.can_undo());
    }

    #[test]
    fn delete_selected_leaf_with_no_selection_returns_false() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let _ = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        // Selection stays None (add_root_leaf does not auto-select).
        assert_eq!(doc.selection(), Selection::None);
        assert!(!doc.delete_selected_leaf().unwrap());
        // Only the AddLeaf entry from the setup is on the stack.
        let undo_entries_before = doc.undo_stack.borrow().len();
        assert_eq!(undo_entries_before, 1);
    }

    #[test]
    fn delete_selected_leaf_of_a_parented_inline_mesh_leaf_succeeds_and_round_trips() {
        // graph_with_one_object is the OBJ-loader shape: an inline-mesh leaf
        // parented under an identity-Group root. The delete primitive must
        // handle this case, and undo must restore the leaf as a child of the
        // same parent.
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let leaf = first_leaf_id(&doc);
        doc.set_selection(Selection::One(leaf));
        let parent_before = doc
            .with_scene(|s| s.graph().parent_of(leaf.node_id()))
            .flatten();

        assert!(doc.delete_selected_leaf().unwrap());

        let leaf_gone = doc
            .with_scene(|s| s.graph().node(leaf.node_id()).is_none())
            .unwrap();
        assert!(leaf_gone);
        assert_eq!(doc.selection_primary_node_id(), None);
        assert!(doc.can_undo());

        doc.undo().unwrap();

        let restored = doc
            .with_scene(|s| {
                (
                    s.graph().node(leaf.node_id()).is_some(),
                    s.graph().parent_of(leaf.node_id()),
                )
            })
            .unwrap();
        assert_eq!(restored, (true, parent_before));
    }

    #[test]
    fn delete_then_undo_restores_the_leaf_name() {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let leaf = g
            .add_leaf(
                Some(sc_scene::SceneMetadata {
                    name: Some("Named".to_string()),
                    comment: None,
                }),
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(IndexedMesh::new(None, Vec::new(), Vec::new()))),
            )
            .expect("add leaf");
        g.add_child(root, leaf).expect("attach leaf");
        let doc = SceneDocument::new();
        doc.install_graph(g, fixture_path());
        let leaf = first_leaf_id(&doc);
        doc.set_selection(Selection::One(leaf));

        assert!(doc.delete_selected_leaf().unwrap());
        doc.undo().unwrap();

        let name = doc
            .with_scene(|s| {
                s.graph()
                    .node(leaf.node_id())
                    .and_then(|node| node.meta.as_ref())
                    .and_then(|meta| meta.name.clone())
            })
            .flatten();
        assert_eq!(name.as_deref(), Some("Named"));
    }

    #[test]
    fn delete_selected_leaf_with_shared_rid_emits_plain_remove_leaf() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let kept = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        let to_delete = doc.add_root_leaf(rid, translation(5.0)).unwrap().unwrap();
        doc.set_selection(Selection::One(crate::selection::LeafId(to_delete)));

        assert!(doc.delete_selected_leaf().unwrap());

        let observed = doc
            .with_scene(|s| {
                (
                    s.graph().node(to_delete).is_some(),
                    s.graph().node(kept).is_some(),
                    s.graph().mesh_resources_iter().count(),
                )
            })
            .unwrap();
        assert_eq!(observed, (false, true, 1));
        assert_eq!(doc.selection_primary_node_id(), None);

        // Undo restores only the leaf; the resource was never removed because
        // the sibling kept it live.
        doc.undo().unwrap();
        let observed = doc
            .with_scene(|s| {
                (
                    s.graph().node(to_delete).is_some(),
                    s.graph().mesh_resources_iter().count(),
                )
            })
            .unwrap();
        assert_eq!(observed, (true, 1));
    }

    #[test]
    fn delete_selected_leaf_as_sole_reference_emits_compound_that_gcs_resource() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let id = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        doc.set_selection(Selection::One(crate::selection::LeafId(id)));

        assert!(doc.delete_selected_leaf().unwrap());

        // Leaf gone and resource arena's live-iter count dropped to 0: the
        // Compound also tombstoned the mesh resource.
        let observed = doc
            .with_scene(|s| {
                (
                    s.graph().node(id).is_some(),
                    s.graph().mesh_resources_iter().count(),
                )
            })
            .unwrap();
        assert_eq!(observed, (false, 0));
        assert_eq!(doc.selection_primary_node_id(), None);

        // Undo restores both leaf and resource via the inverse Compound
        // (Compound([AddMeshResource, AddLeaf])).
        doc.undo().unwrap();
        let observed = doc
            .with_scene(|s| {
                let node_back = s.graph().node(id);
                let kind_ok = matches!(
                    node_back.map(|n| &n.kind),
                    Some(NodeKind::Leaf(LeafPayload::MeshResource(r))) if *r == rid
                );
                (
                    node_back.is_some(),
                    kind_ok,
                    s.graph().mesh_resources_iter().count(),
                )
            })
            .unwrap();
        assert_eq!(observed, (true, true, 1));
    }

    #[test]
    fn delete_selected_leaf_fires_restructured_selection_changed_and_undo_stack_changed() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        let id = doc
            .add_root_leaf(rid, Transform3D::Identity)
            .unwrap()
            .unwrap();
        doc.set_selection(Selection::One(crate::selection::LeafId(id)));
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);
        assert!(doc.delete_selected_leaf().unwrap());
        assert_eq!(
            *log.borrow(),
            vec![
                SceneEvent::Restructured,
                SceneEvent::SelectionChanged,
                SceneEvent::UndoStackChanged,
            ]
        );
    }

    fn triangle_arc() -> Arc<IndexedMesh> {
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
        Arc::new(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        })
    }

    #[test]
    fn paste_fragment_with_no_scene_loaded_returns_none() {
        let doc = SceneDocument::new();
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        assert!(
            doc.paste_fragment(&fragment, [0.0, 0.0, 0.0])
                .unwrap()
                .is_none()
        );
        assert!(!doc.can_undo());
    }

    #[test]
    fn paste_fragment_into_fresh_scene_emits_compound_and_installs_root() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let mesh = triangle_arc();
        let fragment = SceneFragment::from_leaf_snapshot(Arc::clone(&mesh), Transform3D::Identity);
        let new_root = doc
            .paste_fragment(&fragment, [5.0, 0.0, 0.0])
            .unwrap()
            .expect("paste into loaded scene");
        // Both AddMeshResource and AddLeaf landed: the scene has a new root
        // leaf referencing a new resource that shares the same Arc bytes as the
        // clipboard's mesh.
        let observed = doc.with_scene(|s| {
            let node = s.graph().node(new_root).expect("new root lives");
            let NodeKind::Leaf(LeafPayload::MeshResource(rid)) = node.kind else {
                panic!("expected mesh-resource leaf");
            };
            let arc_match = std::ptr::eq(
                Arc::as_ptr(&mesh),
                Arc::as_ptr(s.graph().mesh_resource_arc(rid).unwrap()),
            );
            (
                arc_match,
                s.graph().roots().contains(&new_root),
                node.local_transform,
            )
        });
        let (arc_match, is_root, xform) = observed.unwrap();
        assert!(
            arc_match,
            "target scene must share the Arc with the clipboard"
        );
        assert!(is_root);
        let Transform3D::Affine(a) = xform else {
            panic!("paste must materialise an Affine transform");
        };
        assert_eq!(*a.translation(), [5.0, 0.0, 0.0]);
    }

    #[test]
    fn paste_appends_the_new_root_and_undo_restores_the_root_order() {
        let doc = SceneDocument::new();
        let (graph, rid) = graph_with_one_resource();
        doc.install_graph(graph, fixture_path());
        doc.add_root_leaf(rid, Transform3D::Identity).unwrap();
        doc.add_root_leaf(rid, translation(5.0)).unwrap();
        let roots_before = doc.with_scene(|s| s.graph().roots().to_vec()).unwrap();
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let new_root = doc
            .paste_fragment(&fragment, [9.0, 0.0, 0.0])
            .unwrap()
            .expect("paste into loaded scene");
        let mut expected = roots_before.clone();
        expected.push(new_root);
        assert_eq!(
            doc.with_scene(|s| s.graph().roots().to_vec()).unwrap(),
            expected
        );
        doc.undo().unwrap();
        assert_eq!(
            doc.with_scene(|s| s.graph().roots().to_vec()).unwrap(),
            roots_before
        );
    }

    #[test]
    fn paste_fragment_into_scene_that_shares_the_arc_skips_add_mesh_resource() {
        // Same-scene paste: register the mesh resource ahead of time (mimicking
        // what a prior paste landed) and confirm a second paste with a fragment
        // built from the same Arc reuses that rid via the
        // find_mesh_resource_by_arc dedup.
        let doc = SceneDocument::new();
        let mesh = triangle_arc();
        let mut graph = empty_graph();
        let existing_rid = graph.add_mesh_resource_arc(Arc::clone(&mesh));
        doc.install_graph(graph, fixture_path());
        let resources_before = doc.with_scene(|s| s.graph().mesh_resources_len()).unwrap();
        let fragment = SceneFragment::from_leaf_snapshot(Arc::clone(&mesh), Transform3D::Identity);
        let new_root = doc
            .paste_fragment(&fragment, [0.0, 0.0, 0.0])
            .unwrap()
            .unwrap();
        let (resources_after, leaf_rid) = doc
            .with_scene(|s| {
                let NodeKind::Leaf(LeafPayload::MeshResource(rid)) =
                    s.graph().node(new_root).unwrap().kind
                else {
                    panic!("expected mesh-resource leaf");
                };
                (s.graph().mesh_resources_len(), rid)
            })
            .unwrap();
        assert_eq!(
            resources_after, resources_before,
            "dedupe path must not grow the mesh-resource arena"
        );
        assert_eq!(
            leaf_rid, existing_rid,
            "the pasted leaf must reuse the existing rid"
        );
    }

    #[test]
    fn paste_fragment_records_a_single_undoable_compound() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        assert!(!doc.can_undo());
        let new_root = doc
            .paste_fragment(&fragment, [0.0, 0.0, 0.0])
            .unwrap()
            .unwrap();
        assert!(doc.can_undo());
        // 1 undo reverses the entire paste: leaf and resource both tombstoned,
        // no stranded entry left on the stack. Counts use the live-only
        // iterators because the arena Vecs keep their length even when every
        // entry is `None`.
        doc.undo().unwrap();
        let observed = doc.with_scene(|s| {
            (
                s.graph().node(new_root).is_some(),
                s.graph().mesh_resources_iter().count(),
            )
        });
        assert_eq!(observed, Some((false, 0)));
        assert!(!doc.can_undo());
    }

    #[test]
    fn paste_fragment_fires_restructured_event() {
        // Paste lands a compound that grows the node + mesh-resource arenas, so
        // the document must signal the structural change so the renderer's
        // GpuScene mirror is re-uploaded. A bare `Mutated` event would skip the
        // upload and the next render would trip the leaf-count assertion in
        // mesh_render::render.
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);
        doc.paste_fragment(&fragment, [0.0, 0.0, 0.0])
            .unwrap()
            .unwrap();
        assert!(log.borrow().contains(&SceneEvent::Restructured));
        assert!(!log.borrow().contains(&SceneEvent::Mutated));
    }

    #[test]
    fn set_local_transform_still_fires_mutated_not_restructured() {
        // A non-structural edit (gizmo tick) must keep firing `Mutated` so the
        // upload-owed flag is not flipped on every transform frame, which would
        // burn GPU bandwidth on a hot path that needs none.
        let doc = SceneDocument::new();
        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = first_leaf_id(&doc).node_id();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);
        doc.apply_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        })
        .unwrap();
        assert!(log.borrow().contains(&SceneEvent::Mutated));
        assert!(!log.borrow().contains(&SceneEvent::Restructured));
    }

    #[test]
    fn paste_fragment_selects_the_new_root() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let new_root = doc
            .paste_fragment(&fragment, [0.0, 0.0, 0.0])
            .unwrap()
            .unwrap();
        assert_eq!(doc.selection_primary_node_id(), Some(new_root));
    }

    #[test]
    fn paste_fragment_redo_replays_the_compound() {
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let new_root = doc
            .paste_fragment(&fragment, [1.0, 2.0, 3.0])
            .unwrap()
            .unwrap();
        doc.undo().unwrap();
        doc.redo().unwrap();
        let observed = doc.with_scene(|s| {
            let node = s.graph().node(new_root).expect("redo restores the leaf");
            (
                matches!(node.kind, NodeKind::Leaf(LeafPayload::MeshResource(_))),
                node.local_transform,
            )
        });
        let (kind_ok, xform) = observed.unwrap();
        assert!(kind_ok);
        let Transform3D::Affine(a) = xform else {
            panic!();
        };
        assert_eq!(*a.translation(), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn substitute_translation_preserves_linear_part() {
        let linear = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let original = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            linear,
            [9.0, 9.0, 9.0],
        ));
        let updated = substitute_translation(original, [1.0, 2.0, 3.0]);
        let Transform3D::Affine(a) = updated else {
            panic!();
        };
        assert_eq!(*a.linear(), linear);
        assert_eq!(*a.translation(), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn substitute_translation_promotes_identity_to_pure_translation() {
        let updated = substitute_translation(Transform3D::Identity, [4.0, 5.0, 6.0]);
        let Transform3D::Affine(a) = updated else {
            panic!();
        };
        assert_eq!(*a.translation(), [4.0, 5.0, 6.0]);
    }

    #[test]
    fn a_pasted_mesh_is_swapped_through_its_resource_and_undone() {
        // A pasted fragment (how generated supports enter the scene) holds its
        // mesh through a resource, not inline.
        let doc = SceneDocument::new();
        doc.install_graph(empty_graph(), fixture_path());
        let pasted = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        let fragment = SceneFragment::from_leaf_snapshot(pasted.clone(), Transform3D::Identity);
        doc.paste_fragment(&fragment, [0.0; 3]).unwrap();
        let mesh_of = |doc: &SceneDocument| {
            doc.with_scene(|s| {
                let leaf = s.leaf_id_at_index(0).unwrap();
                (leaf, s.graph().leaf_mesh_arc(leaf).unwrap().clone())
            })
            .unwrap()
        };
        let (leaf, before) = mesh_of(&doc);
        assert!(Arc::ptr_eq(&before, &pasted));
        let after = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        let swap = doc
            .with_scene(|s| leaf_mesh_swap(s.graph(), leaf, after.clone()))
            .flatten()
            .expect("a resource leaf has a swap");
        assert!(matches!(swap, SceneEdit::ReplaceMeshResource { .. }));
        doc.apply_edit(swap.clone()).unwrap();
        doc.push_edit(swap);
        assert!(Arc::ptr_eq(&mesh_of(&doc).1, &after));
        doc.undo().unwrap();
        assert!(Arc::ptr_eq(&mesh_of(&doc).1, &pasted));
    }
}
