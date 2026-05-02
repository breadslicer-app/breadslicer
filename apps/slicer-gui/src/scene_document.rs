/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level model for the loaded scene.
//!
//! Owns the scene and (eventually) its source path, the selection, and
//! the undo/redo command stack. Consumers like the viewport controller
//! and the object-list panel observe the document instead of holding
//! their own copies of this state.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use sc_mesh_core::scene::{Scene, Transform3D};

use crate::selection::{Selection, SelectionState};

/// Maximum number of entries kept on each of the undo and redo stacks.
/// On overflow the oldest entry (front of the deque) is discarded.
pub(crate) const UNDO_STACK_CAP: usize = 256;

/// Coarse-grained dispatch hint for document listeners. Each public
/// mutation on `SceneDocument` fires exactly one event variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SceneEvent {
    /// A scene has just been installed (replacing whatever was there).
    /// Selection has been reset to `None` and `source_path` is set.
    Installed,
    /// The scene has been removed; no scene is currently loaded.
    Cleared,
    /// Selection has changed without the scene itself changing.
    SelectionChanged,
    /// The scene's contents were mutated through `apply_command`.
    Mutated,
    /// The undo or redo stack changed (push, undo, redo, or
    /// install/clear when the stacks were non-empty). Listeners that
    /// only care about scene contents can ignore this; the controller
    /// uses it to mirror `can-undo` / `can-redo` to the UI.
    UndoStackChanged,
}

/// One reversible scene edit. Pushed onto the document's undo stack at
/// gesture-end (drag release, keyboard-burst flush) and consumed by
/// `undo` / `redo` to apply the corresponding state change.
///
/// Single variant for now — the entire current GUI only edits a
/// single object's `transform`. Future commands (add / remove /
/// reorder objects) slot in as additional variants without breaking
/// callers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SceneEdit {
    /// `objects[object_idx].transform` was changed from `before` to
    /// `after`. Applying the edit forward writes `after`; undoing
    /// writes `before`.
    ObjectTransform {
        object_idx: usize,
        before: Transform3D,
        after: Transform3D,
    },
}

impl SceneEdit {
    /// True if applying this edit would not change the scene. The
    /// document filters such no-ops at `push_edit` time so a gesture
    /// that ended exactly where it started doesn't waste a stack slot.
    fn is_noop(&self) -> bool {
        match self {
            Self::ObjectTransform { before, after, .. } => before == after,
        }
    }
}

type Listener = Rc<dyn Fn(SceneEvent)>;

pub(crate) struct SceneDocument {
    scene: RefCell<Option<Scene>>,
    /// Filesystem path the loaded scene came from. Used today to
    /// derive default export filenames; tomorrow for "Save" / "Reload".
    /// Cleared in lockstep with `scene`.
    source_path: RefCell<Option<PathBuf>>,
    /// Currently-selected object. Reset to `Selection::None` whenever
    /// the scene is installed or cleared — a new scene has its own
    /// object indices, so a prior selection is no longer meaningful.
    selection: SelectionState,
    /// Registered listeners. Cloned out of the cell before dispatch so
    /// a listener can re-enter `subscribe` without re-borrowing.
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_listener_id: Cell<u64>,
    /// Reversible edits the user can roll back via Edit → Undo. Most
    /// recent edit at the back; capped at `UNDO_STACK_CAP`.
    undo_stack: RefCell<VecDeque<SceneEdit>>,
    /// Edits that were rolled back by `undo` and can be replayed by
    /// `redo`. Cleared whenever a fresh edit lands via `push_edit` —
    /// new history invalidates the redo trail.
    redo_stack: RefCell<VecDeque<SceneEdit>>,
}

impl SceneDocument {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            scene: RefCell::new(None),
            source_path: RefCell::new(None),
            selection: SelectionState::new(),
            listeners: RefCell::new(Vec::new()),
            next_listener_id: Cell::new(0),
            undo_stack: RefCell::new(VecDeque::new()),
            redo_stack: RefCell::new(VecDeque::new()),
        })
    }

    pub fn install_scene(&self, scene: Scene, source_path: PathBuf) {
        *self.scene.borrow_mut() = Some(scene);
        *self.source_path.borrow_mut() = Some(source_path);
        self.selection.set(Selection::None);
        // Object indices in the new scene have no relationship to the
        // outgoing one's, so any pending edits would target the wrong
        // objects on replay.
        let stacks_changed = self.clear_history();
        self.notify(SceneEvent::Installed);
        if stacks_changed {
            self.notify(SceneEvent::UndoStackChanged);
        }
    }

    pub fn clear_scene(&self) {
        *self.scene.borrow_mut() = None;
        *self.source_path.borrow_mut() = None;
        self.selection.set(Selection::None);
        let stacks_changed = self.clear_history();
        self.notify(SceneEvent::Cleared);
        if stacks_changed {
            self.notify(SceneEvent::UndoStackChanged);
        }
    }

    /// Drops any pending undo/redo entries. Returns whether either
    /// stack was non-empty before the call — callers that fire
    /// `UndoStackChanged` only when an observable change happened use
    /// this to gate the notification.
    fn clear_history(&self) -> bool {
        let mut undo = self.undo_stack.borrow_mut();
        let mut redo = self.redo_stack.borrow_mut();
        let had = !undo.is_empty() || !redo.is_empty();
        undo.clear();
        redo.clear();
        had
    }

    pub fn selection(&self) -> Selection {
        self.selection.current()
    }

    pub fn set_selection(&self, sel: Selection) {
        if self.selection.current() == sel {
            return;
        }
        self.selection.set(sel);
        self.notify(SceneEvent::SelectionChanged);
    }

    /// Currently-selected object's index as a plain `usize`. Convenience
    /// for the (very common) call sites that only want to look up
    /// `scene.objects[idx]`.
    pub fn selection_primary_idx(&self) -> Option<usize> {
        self.selection.primary_idx()
    }

    /// Mutation chokepoint for command-pattern writes. Wraps the
    /// closure's run in a `Mutated` notification so the controller's
    /// listener (and any future undo stack) sees every mutation in a
    /// single place. Returns `Some(R)` when a scene was loaded,
    /// `None` otherwise.
    pub fn apply_command<R, F: FnOnce(&mut Scene) -> R>(&self, cmd: F) -> Option<R> {
        let result = self.with_scene_mut(cmd)?;
        self.notify(SceneEvent::Mutated);
        Some(result)
    }

    /// Records a reversible edit on the undo stack. No-op edits
    /// (`before == after`) are dropped silently so a gesture that
    /// began and ended in the same place doesn't clutter the history.
    /// A successful push clears the redo stack — a fresh edit
    /// invalidates the previously-rolled-back trail.
    pub(crate) fn push_edit(&self, edit: SceneEdit) {
        if edit.is_noop() {
            return;
        }
        {
            let mut undo = self.undo_stack.borrow_mut();
            if undo.len() == UNDO_STACK_CAP {
                undo.pop_front();
            }
            undo.push_back(edit);
        }
        self.redo_stack.borrow_mut().clear();
        self.notify(SceneEvent::UndoStackChanged);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.borrow().is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.borrow().is_empty()
    }

    /// Pops the most recent edit, applies its inverse to the scene,
    /// and moves the entry onto the redo stack. Returns `true` if an
    /// edit was applied. The inverse is written through `apply_command`
    /// so the existing `Mutated` listener schedules the redraw;
    /// `UndoStackChanged` fires afterwards for the menu state.
    pub fn undo(&self) -> bool {
        let Some(edit) = self.undo_stack.borrow_mut().pop_back() else {
            return false;
        };
        let applied = self
            .apply_command(|scene| apply_edit_inverse(scene, edit))
            .is_some();
        if !applied {
            // No scene loaded — restore the entry we just popped so
            // the stack stays consistent with the user's history.
            self.undo_stack.borrow_mut().push_back(edit);
            return false;
        }
        let mut redo = self.redo_stack.borrow_mut();
        if redo.len() == UNDO_STACK_CAP {
            redo.pop_front();
        }
        redo.push_back(edit);
        drop(redo);
        self.notify(SceneEvent::UndoStackChanged);
        true
    }

    /// Pops the most recent rolled-back edit, applies it forward, and
    /// moves the entry back onto the undo stack. Returns `true` if an
    /// edit was applied.
    pub fn redo(&self) -> bool {
        let Some(edit) = self.redo_stack.borrow_mut().pop_back() else {
            return false;
        };
        let applied = self
            .apply_command(|scene| apply_edit_forward(scene, edit))
            .is_some();
        if !applied {
            self.redo_stack.borrow_mut().push_back(edit);
            return false;
        }
        let mut undo = self.undo_stack.borrow_mut();
        if undo.len() == UNDO_STACK_CAP {
            undo.pop_front();
        }
        undo.push_back(edit);
        drop(undo);
        self.notify(SceneEvent::UndoStackChanged);
        true
    }

    /// Registers a listener. Returns a handle whose `Drop` removes the
    /// listener. Listeners fire in registration order.
    pub fn subscribe<F>(self: &Rc<Self>, listener: F) -> SceneSubscription
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
        // Snapshot listeners so a listener body can re-enter
        // `subscribe` / `unsubscribe` without re-borrowing the cell.
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

    /// Path the current scene was loaded from. `None` when no scene is
    /// loaded; cloned out so the caller doesn't extend the borrow.
    // Tests exercise the field; the production caller (export-as
    // default-filename derivation) lands with the export feature.
    #[cfg(test)]
    pub fn source_path(&self) -> Option<PathBuf> {
        self.source_path.borrow().clone()
    }

    /// Borrow form for callers that only need the path's lifetime
    /// inside a closure (avoids the clone in `source_path`).
    pub fn with_source_path<R>(&self, f: impl FnOnce(&Path) -> R) -> Option<R> {
        self.source_path.borrow().as_deref().map(f)
    }

    /// Runs `f` against the loaded scene, if any. Returns `None` when
    /// no scene is installed. Callers must keep the closure short — the
    /// internal borrow is held for its duration.
    pub fn with_scene<R>(&self, f: impl FnOnce(&Scene) -> R) -> Option<R> {
        self.scene.borrow().as_ref().map(f)
    }

    /// Mutable counterpart to `with_scene`. Same borrow-scope rules
    /// apply; nesting another scene access (read or write) inside `f`
    /// will panic on the inner borrow.
    ///
    /// Production mutations should go through `apply_command` so the
    /// `Mutated` event fires; this method is the bypass exception
    /// (currently only the unit tests use it directly).
    pub(crate) fn with_scene_mut<R>(&self, f: impl FnOnce(&mut Scene) -> R) -> Option<R> {
        self.scene.borrow_mut().as_mut().map(f)
    }

    pub fn is_loaded(&self) -> bool {
        self.scene.borrow().is_some()
    }
}

/// Writes the `after` state of `edit` into `scene`. Out-of-range
/// indices are silently ignored — the edit's target object may have
/// been removed by a future command between the push and the replay.
fn apply_edit_forward(scene: &mut Scene, edit: SceneEdit) {
    match edit {
        SceneEdit::ObjectTransform {
            object_idx, after, ..
        } => {
            if let Some(obj) = scene.objects.get_mut(object_idx) {
                obj.transform = after;
            }
        }
    }
}

/// Writes the `before` state of `edit` into `scene` — the inverse of
/// `apply_edit_forward`.
fn apply_edit_inverse(scene: &mut Scene, edit: SceneEdit) {
    match edit {
        SceneEdit::ObjectTransform {
            object_idx, before, ..
        } => {
            if let Some(obj) = scene.objects.get_mut(object_idx) {
                obj.transform = before;
            }
        }
    }
}

/// RAII handle to a `SceneDocument` subscription. Drop to unsubscribe.
pub(crate) struct SceneSubscription {
    id: u64,
    document: Weak<SceneDocument>,
}

impl Drop for SceneSubscription {
    fn drop(&mut self) {
        let Some(doc) = self.document.upgrade() else {
            return;
        };
        doc.listeners.borrow_mut().retain(|(id, _)| *id != self.id);
    }
}

#[cfg(test)]
mod scene_document_tests {
    use super::*;
    use sc_mesh_core::scene::{SceneObject, SceneObjectContent};

    fn empty_scene() -> Scene {
        Scene::new(None, Vec::new(), Vec::new())
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from("/tmp/scene_document_fixture.obj")
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
    fn install_scene_makes_it_observable() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        assert!(doc.is_loaded());
        let object_count = doc.with_scene(|s| s.objects().len());
        assert_eq!(object_count, Some(0));
        assert_eq!(doc.source_path(), Some(fixture_path()));
    }

    #[test]
    fn clear_scene_removes_it() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        doc.clear_scene();
        assert!(!doc.is_loaded());
        assert!(doc.with_scene(|_| ()).is_none());
        assert!(doc.source_path().is_none());
    }

    #[test]
    fn install_scene_replaces_a_prior_scene() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), PathBuf::from("/tmp/a.obj"));
        doc.install_scene(empty_scene(), PathBuf::from("/tmp/b.obj"));
        assert!(doc.is_loaded());
        assert_eq!(doc.source_path(), Some(PathBuf::from("/tmp/b.obj")));
    }

    #[test]
    fn with_scene_mut_observes_writes_through_with_scene() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        let added = doc.with_scene_mut(|s| {
            s.objects.push(SceneObject {
                meta: None,
                transform: Default::default(),
                content: SceneObjectContent::Resource(0),
            });
            s.objects.len()
        });
        assert_eq!(added, Some(1));
        let observed = doc.with_scene(|s| s.objects.len());
        assert_eq!(observed, Some(1));
    }

    #[test]
    fn with_scene_mut_on_empty_returns_none() {
        let doc = SceneDocument::new();
        let result = doc.with_scene_mut(|s| s.objects.len());
        assert!(result.is_none());
    }

    #[test]
    fn with_source_path_runs_only_when_a_path_is_set() {
        let doc = SceneDocument::new();
        assert!(doc.with_source_path(|_| 0_usize).is_none());
        doc.install_scene(empty_scene(), fixture_path());
        let stem = doc.with_source_path(|p| p.file_stem().unwrap().to_owned());
        assert_eq!(stem.as_deref(), Some("scene_document_fixture".as_ref()));
    }

    #[test]
    fn fresh_document_has_no_selection() {
        let doc = SceneDocument::new();
        assert_eq!(doc.selection(), Selection::None);
        assert_eq!(doc.selection_primary_idx(), None);
    }

    #[test]
    fn set_selection_round_trips() {
        use crate::selection::ObjectIdx;
        let doc = SceneDocument::new();
        doc.set_selection(Selection::One(ObjectIdx(2)));
        assert_eq!(doc.selection(), Selection::One(ObjectIdx(2)));
        assert_eq!(doc.selection_primary_idx(), Some(2));
    }

    #[test]
    fn install_scene_resets_selection() {
        use crate::selection::ObjectIdx;
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        doc.set_selection(Selection::One(ObjectIdx(7)));
        // Loading another scene drops the prior selection — its index
        // referred to the previous scene's objects.
        doc.install_scene(empty_scene(), fixture_path());
        assert_eq!(doc.selection(), Selection::None);
    }

    #[test]
    fn clear_scene_resets_selection() {
        use crate::selection::ObjectIdx;
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        doc.set_selection(Selection::One(ObjectIdx(0)));
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

        doc.install_scene(empty_scene(), fixture_path());
        doc.clear_scene();

        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::Installed, SceneEvent::Cleared]
        );
    }

    #[test]
    fn set_selection_only_fires_on_actual_change() {
        use crate::selection::ObjectIdx;
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.set_selection(Selection::None); // no change from initial
        doc.set_selection(Selection::One(ObjectIdx(0))); // changes
        doc.set_selection(Selection::One(ObjectIdx(0))); // no change
        doc.set_selection(Selection::None); // changes back

        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::SelectionChanged, SceneEvent::SelectionChanged]
        );
    }

    #[test]
    fn apply_command_fires_mutated_when_a_scene_is_loaded() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        // No scene loaded → command is a no-op and no event fires.
        let pre = doc.apply_command(|s| s.objects.len());
        assert_eq!(pre, None);

        doc.install_scene(empty_scene(), fixture_path());
        let post = doc.apply_command(|s| {
            s.objects.push(SceneObject {
                meta: None,
                transform: Default::default(),
                content: SceneObjectContent::Resource(0),
            });
            s.objects.len()
        });
        assert_eq!(post, Some(1));

        // Installed (from install_scene), then Mutated (from apply_command).
        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::Installed, SceneEvent::Mutated]
        );
    }

    #[test]
    fn dropping_subscription_removes_the_listener() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        {
            let _sub = doc.subscribe(listener);
            doc.install_scene(empty_scene(), fixture_path());
        } // subscription drops here
        doc.clear_scene();

        assert_eq!(*log.borrow(), vec![SceneEvent::Installed]);
    }

    use sc_mesh_core::transform::AffineTransform3D;

    fn translation(dx: f64) -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation([dx, 0.0, 0.0]))
    }

    fn scene_with_one_object() -> Scene {
        let mut s = empty_scene();
        s.objects.push(SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Resource(0),
        });
        s
    }

    fn obj_transform(doc: &SceneDocument, idx: usize) -> Transform3D {
        doc.with_scene(|s| s.objects[idx].transform).unwrap()
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
        doc.install_scene(scene_with_one_object(), fixture_path());
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn push_edit_skips_no_op() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        });
        assert!(!doc.can_undo());
    }

    #[test]
    fn undo_restores_before_and_enables_redo() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        // Simulate a gesture: write `after`, then record the edit.
        let after = translation(5.0);
        doc.apply_command(|s| s.objects[0].transform = after);
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after,
        });

        assert!(doc.undo());
        assert_eq!(obj_transform(&doc, 0), Transform3D::Identity);
        assert!(!doc.can_undo());
        assert!(doc.can_redo());
    }

    #[test]
    fn redo_replays_after_and_re_enables_undo() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        let after = translation(5.0);
        doc.apply_command(|s| s.objects[0].transform = after);
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after,
        });

        doc.undo();
        assert!(doc.redo());
        assert_eq!(obj_transform(&doc, 0), after);
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn fresh_push_clears_redo_trail() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        let a = translation(1.0);
        doc.apply_command(|s| s.objects[0].transform = a);
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: a,
        });
        doc.undo();
        assert!(doc.can_redo());

        // A new branch of history invalidates the redo trail.
        let b = translation(2.0);
        doc.apply_command(|s| s.objects[0].transform = b);
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: b,
        });
        assert!(doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn undo_and_redo_are_no_ops_when_stacks_are_empty() {
        let doc = SceneDocument::new();
        assert!(!doc.undo());
        assert!(!doc.redo());
        doc.install_scene(empty_scene(), fixture_path());
        assert!(!doc.undo());
        assert!(!doc.redo());
    }

    #[test]
    fn stack_caps_at_undo_stack_cap() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        // Push UNDO_STACK_CAP + 5 distinct edits; the oldest five
        // should be silently dropped by `pop_front`.
        for i in 0..(UNDO_STACK_CAP + 5) {
            let dx = (i + 1) as f64;
            doc.push_edit(SceneEdit::ObjectTransform {
                object_idx: 0,
                before: translation(dx - 1.0),
                after: translation(dx),
            });
        }
        assert_eq!(doc.undo_stack.borrow().len(), UNDO_STACK_CAP);
        // Front of the deque is the oldest surviving entry; with
        // five entries dropped it should start at i = 5 (after = 6.0).
        let oldest = doc.undo_stack.borrow().front().copied().unwrap();
        match oldest {
            SceneEdit::ObjectTransform { after, .. } => {
                assert_eq!(after, translation(6.0));
            }
        }
    }

    #[test]
    fn install_scene_clears_undo_history() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert!(doc.can_undo());
        doc.install_scene(scene_with_one_object(), fixture_path());
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn clear_scene_clears_undo_history() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
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
        doc.install_scene(scene_with_one_object(), fixture_path());
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert_eq!(*log.borrow(), vec![SceneEvent::UndoStackChanged]);
    }

    #[test]
    fn undo_fires_mutated_then_undo_stack_changed() {
        let doc = SceneDocument::new();
        doc.install_scene(scene_with_one_object(), fixture_path());
        let after = translation(1.0);
        doc.apply_command(|s| s.objects[0].transform = after);
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after,
        });
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        doc.undo();
        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::Mutated, SceneEvent::UndoStackChanged]
        );
    }

    #[test]
    fn install_scene_emits_undo_stack_changed_only_when_history_was_non_empty() {
        let doc = SceneDocument::new();
        let (log, listener) = capture_events();
        let _sub = doc.subscribe(listener);

        // Empty → empty: only `Installed`.
        doc.install_scene(scene_with_one_object(), fixture_path());
        assert_eq!(*log.borrow(), vec![SceneEvent::Installed]);

        // Push an edit, then install again — the second install must
        // also report the history clear.
        doc.push_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        log.borrow_mut().clear();
        doc.install_scene(scene_with_one_object(), fixture_path());
        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::Installed, SceneEvent::UndoStackChanged]
        );
    }
}
