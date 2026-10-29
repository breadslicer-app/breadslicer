/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Autosave and session recovery (#18).
//!
//! Every [`AUTOSAVE_EVERY`], each window whose scene has unsaved changes not
//! yet autosaved writes a `.bread` snapshot of it into this run's folder
//! ([`crate::autosave_store`]), off the UI thread, like a save but never to
//! the scene's own file. The snapshot goes away once the scene is saved,
//! discarded or closed, so what a later start finds is unsaved work from a run
//! that ended without closing its scenes: [`crate::app_state`] offers it back.
//!
//! A scene read from a snapshot, or from an earlier version of a project
//! ([`crate::project_versions`]), stands for the file it came from, with its
//! changes unsaved ([`recovered_identity`]).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use futures_channel::oneshot;

use crate::autosave_store::{
    AutosaveStore, MANIFEST_FORMAT, Manifest, OrphanEntry, manifest_of, unix_seconds,
};
use crate::preferences_bus::PreferencesBus;
use crate::print_selection::PrintDefaults;
use crate::project_versions::ProjectVersions;
use crate::scene_document::{
    DocumentOrigin, SaveStamp, SceneDocument, SceneEvent, SceneSubscription,
};
use crate::viewport::ViewportController;

/// How often a window autosaves unsaved changes.
pub(crate) const AUTOSAVE_EVERY: Duration = Duration::from_secs(120);

/// The file a scene read from `path` stands for, and how it was loaded, when
/// `path` is an autosave or an earlier version of a project.
pub(crate) fn recovered_identity(path: &Path) -> Option<(PathBuf, DocumentOrigin)> {
    if let Some(root) = AutosaveStore::platform_root()
        && let Some(manifest) = manifest_of(&root, path)
    {
        let origin = if manifest.project {
            DocumentOrigin::Project
        } else {
            DocumentOrigin::PlainMesh
        };
        return Some((manifest.source, origin));
    }
    let source = ProjectVersions::platform_default()?.project_of(path)?;
    Some((source, DocumentOrigin::Project))
}

/// The folder of the earlier versions of the project `doc` holds, when it is
/// a `.bread` project with any.
pub(crate) fn versions_dir_of(doc: &SceneDocument) -> Option<PathBuf> {
    if !crate::scene_save::saves_in_place(doc) {
        return None;
    }
    let source = doc.with_source_path(Path::to_path_buf)?;
    ProjectVersions::platform_default()?.existing_dir_for(&source)
}

/// The line that names 1 recoverable scene: its file and when it was
/// autosaved, as the time alone for today.
pub(crate) fn recovery_line(entry: &OrphanEntry, now: SystemTime) -> String {
    use chrono::{DateTime, Local};

    let name = entry
        .manifest
        .source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let saved: DateTime<Local> =
        (SystemTime::UNIX_EPOCH + Duration::from_secs(entry.manifest.saved_at)).into();
    let now: DateTime<Local> = now.into();
    let when = if saved.date_naive() == now.date_naive() {
        saved.format("%H:%M")
    } else {
        saved.format("%Y-%m-%d %H:%M")
    };
    format!("{name} — {when}")
}

/// Autosaves 1 window's scene.
pub(crate) struct Autosaver {
    window: u64,
    store: Rc<AutosaveStore>,
    ctrl: Weak<ViewportController>,
    prefs: Rc<PreferencesBus>,
    /// The document state the entry on disk holds; `None` when there is none.
    written: Cell<Option<SaveStamp>>,
    /// A snapshot is being written.
    writing: Cell<bool>,
    /// Cleared when the entry must go: a snapshot in flight then takes itself
    /// back instead of landing after the entry was removed.
    wanted: Arc<AtomicBool>,
    /// The window closed: nothing more is written.
    closed: Cell<bool>,
    timer: slint::Timer,
    _subscription: SceneSubscription,
}

impl Autosaver {
    /// Starts autosaving the scene of window `window`, shown by `ctrl`.
    pub(crate) fn install(
        window: u64,
        store: Rc<AutosaveStore>,
        ctrl: &Rc<ViewportController>,
        prefs: Rc<PreferencesBus>,
    ) -> Rc<Self> {
        let doc = ctrl.document();
        let autosaver = Rc::new_cyclic(|me: &Weak<Self>| {
            let me = me.clone();
            let subscription = doc.subscribe(move |event| {
                if let Some(me) = me.upgrade() {
                    me.on_scene_event(event);
                }
            });
            Self {
                window,
                store,
                ctrl: Rc::downgrade(ctrl),
                prefs,
                written: Cell::new(None),
                writing: Cell::new(false),
                wanted: Arc::new(AtomicBool::new(true)),
                closed: Cell::new(false),
                timer: slint::Timer::default(),
                _subscription: subscription,
            }
        });
        let me = Rc::downgrade(&autosaver);
        autosaver
            .timer
            .start(slint::TimerMode::Repeated, AUTOSAVE_EVERY, move || {
                if let Some(me) = me.upgrade() {
                    me.tick();
                }
            });
        autosaver
    }

    /// The window is closing: its entry goes, and nothing more is written.
    pub(crate) fn close(&self) {
        self.closed.set(true);
        self.timer.stop();
        self.forget();
    }

    /// Removes the window's entry: its scene has nothing left to recover.
    fn forget(&self) {
        self.wanted.store(false, Ordering::SeqCst);
        self.written.set(None);
        self.store.remove_entry(self.window);
    }

    fn on_scene_event(&self, event: SceneEvent) {
        let Some(ctrl) = self.ctrl.upgrade() else {
            return;
        };
        match event {
            SceneEvent::Cleared => self.forget(),
            SceneEvent::DirtyChanged if !ctrl.document().is_dirty() => self.forget(),
            _ => {}
        }
    }

    fn tick(self: &Rc<Self>) {
        let Some(ctrl) = self.ctrl.upgrade() else {
            return;
        };
        if self.closed.get() || self.writing.get() {
            return;
        }
        let doc = ctrl.document();
        // A window still loading keeps its entry: a recovered scene is read
        // from it.
        if !doc.is_loaded() {
            return;
        }
        if !doc.is_dirty() {
            self.forget();
            return;
        }
        let stamp = doc.save_stamp();
        if self.written.get() == Some(stamp) || ctrl.is_mid_gesture() {
            return;
        }
        let Some(source) = doc.with_source_path(Path::to_path_buf) else {
            return;
        };
        let slot = match self.store.next_slot(self.window) {
            Ok(slot) => slot,
            Err(err) => {
                crate::breadcrumbs::note(&format!("autosave: no folder: {err}"));
                return;
            }
        };
        let defaults = PrintDefaults::from(&self.prefs.snapshot());
        let Some((bundle, _, _)) =
            crate::scene_save::prepare_save_for_target(&doc, slot.path(), &defaults)
        else {
            return;
        };
        let manifest = Manifest {
            format: MANIFEST_FORMAT,
            snapshot: slot.name().to_owned(),
            source,
            project: doc.is_project(),
            saved_at: unix_seconds(SystemTime::now()),
        };
        self.wanted.store(true, Ordering::SeqCst);
        let wanted = self.wanted.clone();
        self.writing.set(true);
        let (tx, rx) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::scene_save::write_bundle(&bundle, |_, _, _| {})?;
                    if wanted.load(Ordering::SeqCst) {
                        slot.publish(&manifest)?;
                    }
                    // Checked again after publishing: an entry removed
                    // meanwhile must not come back.
                    if !wanted.load(Ordering::SeqCst) {
                        slot.withdraw();
                    }
                    anyhow::Ok(())
                }))
                .unwrap_or_else(|payload| {
                    Err(anyhow::anyhow!(crate::mesh_loader::panic_message(&payload)))
                });
                let _ = slint::invoke_from_event_loop(move || {
                    let _ = tx.send(result);
                });
            });
        if let Err(err) = spawned {
            self.writing.set(false);
            crate::breadcrumbs::note(&format!("autosave: no writer: {err}"));
            return;
        }
        let me = Rc::downgrade(self);
        if let Err(err) = slint::spawn_local(async move {
            let result = rx.await;
            if let Some(me) = me.upgrade() {
                me.written_back(stamp, result);
            }
        }) {
            self.writing.set(false);
            crate::breadcrumbs::note(&format!("autosave: not awaited: {err}"));
        }
    }

    /// Records a finished write of the document state `stamp`.
    fn written_back(
        &self,
        stamp: SaveStamp,
        result: Result<anyhow::Result<()>, oneshot::Canceled>,
    ) {
        self.writing.set(false);
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                crate::breadcrumbs::note(&format!("autosave: not written: {err:#}"));
                return;
            }
            Err(_canceled) => return,
        }
        let Some(ctrl) = self.ctrl.upgrade() else {
            return;
        };
        let doc = ctrl.document();
        if self.closed.get() || !doc.is_dirty() || !doc.save_stamp().same_scene(stamp) {
            // Saved, discarded or replaced while the snapshot was written.
            self.forget();
        } else if self.wanted.load(Ordering::SeqCst) {
            self.written.set(Some(stamp));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
    use sc_mesh_spatial::SceneSpatial;
    use sc_project_formats::{ProjectFileType, ProjectPayload, read_project};
    use sc_scene::{LeafPayload, Scene};

    use crate::scene_save::{
        SaveDispatch, decide_save_dispatch, prepare_save_for_target, write_bundle,
    };

    fn one_object_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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

    /// The issue's "done when": a run interrupted with unsaved changes comes
    /// back, at the next start, as the project it was, unsaved.
    #[test]
    fn an_interrupted_run_s_scene_comes_back_as_its_project_unsaved() {
        let data = tempfile::tempdir().unwrap();
        let project = PathBuf::from("/models/hueco.bread");

        // The run that gets interrupted autosaves its dirty scene.
        let store = AutosaveStore::new(data.path().to_path_buf());
        let doc = SceneDocument::new();
        doc.install_project_graph(
            one_object_graph(),
            project.clone(),
            ProjectPayload::default(),
        );
        let printer = sc_print_primitives::PrinterId::new("picked-printer").unwrap();
        assert!(doc.set_selected_printer(Some(printer.clone())));
        let slot = store.next_slot(0).unwrap();
        let (bundle, _, _) =
            prepare_save_for_target(&doc, slot.path(), &PrintDefaults::default()).unwrap();
        write_bundle(&bundle, |_, _, _| {}).unwrap();
        slot.publish(&Manifest {
            format: MANIFEST_FORMAT,
            snapshot: slot.name().to_owned(),
            source: project.clone(),
            project: doc.is_project(),
            saved_at: unix_seconds(SystemTime::now()),
        })
        .unwrap();
        drop(store);

        // The next start finds it and moves it into its own run.
        let next = AutosaveStore::new(data.path().to_path_buf());
        let mut orphans = next.claim_orphans(SystemTime::now());
        assert_eq!(orphans.len(), 1);
        let orphan = orphans.pop().unwrap();
        let snapshot = next.adopt(&orphan.entries[0], 3).unwrap();
        orphan.discard();

        // The snapshot reads back with the change, and stands for the
        // project.
        let loaded = read_project(
            &mut std::fs::File::open(&snapshot).unwrap(),
            ProjectFileType::Bread,
        )
        .unwrap();
        assert_eq!(loaded.payload.as_ref().unwrap().printer, Some(printer));
        let manifest = manifest_of(data.path(), &snapshot).unwrap();
        assert_eq!(manifest.source, project);
        assert!(manifest.project);

        let recovered = SceneDocument::new();
        recovered.install_spatial(
            SceneSpatial::from_graph(loaded.scene),
            snapshot.clone(),
            loaded.payload,
        );
        recovered.restore_identity(manifest.source, DocumentOrigin::Project);
        assert!(recovered.is_dirty());
        assert_eq!(
            decide_save_dispatch(&recovered),
            SaveDispatch::OverwriteProject(project)
        );
    }
}
