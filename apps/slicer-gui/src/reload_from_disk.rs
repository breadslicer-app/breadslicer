/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Reload a model from disk (#721).
//!
//! A model whose file changed (edited again in a sculpting or CAD program,
//! fixed by its author) comes back into the scene without starting over:
//! the file is read again and its meshes take the place of the models it
//! gave, at the same pose, in one undo step. What was made from them is
//! made again: a model hollowed here is hollowed again with its settings,
//! drains and solid areas; its supports, out of date with the new mesh,
//! are made again by the supports' own watch, keeping what was placed by
//! hand (#526, #408); a model that does not slice is repaired as one that
//! came in (#677).
//!
//! The files of the models in the scene are watched: when one is saved
//! again and holds still a moment, its models reload on their own, with a
//! line in the status bar, unless the setting is off. A file that is gone
//! or no longer opens leaves its models as they are, and says why.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, SystemTime};

use sc_mesh_core::IndexedMesh;
use sc_scene::{CsgPolicy, NodeId, ReferencePolicy};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::hollow_run::{HollowRun, Hollowed};
use crate::i18n::ctr;
use crate::preferences_bus::PreferencesBus;
use crate::scene_document::SceneDocument;
use crate::viewport::ViewportController;

/// How often the files are looked at.
const WATCH_EVERY: Duration = Duration::from_millis(1000);
/// How long a line stays in the status bar.
const NOTICE_FOR: Duration = Duration::from_secs(6);

/// When a file was last written, and how long it is.
type Stamp = (SystemTime, u64);

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The meshes a file gives, as an import reads them: each model with faces,
/// in the file's order.
pub(crate) fn meshes_of_file(path: &Path) -> Result<Vec<Arc<IndexedMesh>>, String> {
    let loaded = sc_project_formats::load_scene(path, None, |_| {}).map_err(|e| e.to_string())?;
    let sc_project_formats::LoadedContent::Mesh(scene) = loaded.content else {
        return Err(ctr!("Reload", "it is a project, not a model's file").to_string());
    };
    let leaves = scene
        .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
        .map_err(|e| e.to_string())?;
    Ok(leaves
        .into_iter()
        .filter_map(|id| scene.leaf_mesh_arc(id).cloned())
        .filter(|mesh| !mesh.faces.is_empty())
        .collect())
}

/// A file being read again, for the models it gave.
struct Job {
    path: PathBuf,
    models: Vec<NodeId>,
    stamp: Option<Stamp>,
    by_hand: bool,
    read: Receiver<Result<Vec<Arc<IndexedMesh>>, String>>,
}

/// Reloads models from their files, by hand and as their files change.
pub(crate) struct ReloadRun {
    document: Rc<SceneDocument>,
    hollow: Rc<HollowRun>,
    bus: Rc<PreferencesBus>,
    ctrl: Rc<ViewportController>,
    ui: slint::Weak<MainWindow>,
    /// Each watched file as it was last seen.
    stamps: RefCell<HashMap<PathBuf, Stamp>>,
    /// A change seen once: it reloads once it holds still a moment.
    changed: RefCell<HashMap<PathBuf, Stamp>>,
    jobs: RefCell<Vec<Job>>,
    /// Models to hollow again once the hollow driver is free.
    rehollow: RefCell<Vec<(NodeId, Arc<IndexedMesh>, Hollowed)>>,
    timer: slint::Timer,
    notice: slint::Timer,
}

impl ReloadRun {
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        hollow: Rc<HollowRun>,
        bus: Rc<PreferencesBus>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            document,
            hollow,
            bus,
            ctrl,
            ui: ui.as_weak(),
            stamps: RefCell::new(HashMap::new()),
            changed: RefCell::new(HashMap::new()),
            jobs: RefCell::new(Vec::new()),
            rehollow: RefCell::new(Vec::new()),
            timer: slint::Timer::default(),
            notice: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&run);
        run.ctrl.set_on_reload(Rc::new(move |models| {
            if let Some(run) = weak.upgrade() {
                run.reload(&models, true);
            }
        }));
        let weak = Rc::downgrade(&run);
        run.timer
            .start(slint::TimerMode::Repeated, WATCH_EVERY, move || {
                if let Some(run) = weak.upgrade() {
                    run.tick();
                }
            });
        run
    }

    /// Reloads the files `models` came from; `by_hand` when asked for.
    pub(crate) fn reload(&self, models: &[NodeId], by_hand: bool) {
        let mut paths: Vec<PathBuf> = Vec::new();
        for &model in models {
            match self.document.reloadable_file(model) {
                Some(path) if !paths.contains(&path) => paths.push(path),
                Some(_) => {}
                None if by_hand => {
                    self.say(ctr!(
                        "Reload",
                        "This model came from no file it can be read again from."
                    ));
                }
                None => {}
            }
        }
        for path in paths {
            self.start(path, by_hand);
        }
    }

    fn start(&self, path: PathBuf, by_hand: bool) {
        if self.jobs.borrow().iter().any(|job| job.path == path) {
            return;
        }
        let models = self.document.models_from_file(&path);
        if models.is_empty() {
            return;
        }
        let stamp = stamp_of(&path);
        let (send, read) = std::sync::mpsc::channel();
        let from = path.clone();
        let spawned = std::thread::Builder::new()
            .name("reload-from-disk".to_owned())
            .spawn(move || {
                let _ = send.send(meshes_of_file(&from));
            });
        if spawned.is_err() {
            return;
        }
        crate::breadcrumbs::note("reload: reading a model's file again");
        self.jobs.borrow_mut().push(Job {
            path,
            models,
            stamp,
            by_hand,
            read,
        });
    }

    fn tick(&self) {
        self.land_jobs();
        self.hollow_waiting();
        self.watch();
    }

    fn land_jobs(&self) {
        let mut done = Vec::new();
        {
            let mut jobs = self.jobs.borrow_mut();
            let mut k = 0;
            while k < jobs.len() {
                match jobs[k].read.try_recv() {
                    Ok(result) => done.push((jobs.remove(k), result)),
                    Err(TryRecvError::Empty) => k += 1,
                    Err(TryRecvError::Disconnected) => {
                        let job = jobs.remove(k);
                        done.push((job, Err(String::new())));
                    }
                }
            }
        }
        for (job, result) in done {
            self.land(job, result);
        }
    }

    fn land(&self, job: Job, result: Result<Vec<Arc<IndexedMesh>>, String>) {
        if let Some(stamp) = job.stamp {
            self.stamps.borrow_mut().insert(job.path.clone(), stamp);
        }
        let name = job
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meshes = match result {
            Ok(meshes) => meshes,
            Err(why) => {
                let head = ctr!("Reload", "{} could not be read again.", name);
                self.say(if why.is_empty() {
                    head
                } else {
                    format!("{head} {why}").into()
                });
                return;
            }
        };
        if meshes.len() != job.models.len() {
            self.say(ctr!(
                "Reload",
                "{} no longer holds the models it gave, so they stay as they are.",
                name
            ));
            return;
        }
        // The models as they stand now, those still in the scene.
        let now: Vec<(NodeId, Arc<IndexedMesh>, Arc<IndexedMesh>)> = job
            .models
            .iter()
            .zip(meshes)
            .filter_map(|(&leaf, after)| {
                let before = self
                    .document
                    .with_scene(|spatial| spatial.graph().leaf_mesh_arc(leaf).cloned())
                    .flatten()?;
                Some((leaf, before, after))
            })
            .collect();
        let hollows: Vec<(NodeId, Arc<IndexedMesh>, Hollowed)> = now
            .iter()
            .filter_map(|(leaf, before, after)| {
                Some((*leaf, after.clone(), self.hollow.record_of(before)?))
            })
            .collect();
        // Each leaf swaps its mesh however it holds it, inline or through a
        // resource another leaf may show too.
        let swaps: Option<Vec<SceneEdit>> = self
            .document
            .with_scene(|spatial| {
                now.into_iter()
                    .map(|(leaf, _, after)| {
                        crate::scene_document::leaf_mesh_swap(spatial.graph(), leaf, after)
                    })
                    .collect()
            })
            .flatten();
        let Some(swaps) = swaps else {
            return;
        };
        let edit = SceneEdit::Compound(swaps);
        if self.document.apply_edit(edit.clone()).is_err() {
            if job.by_hand {
                self.say(ctr!("Reload", "{} could not be reloaded.", name));
            }
            return;
        }
        self.document.push_edit(edit);
        crate::breadcrumbs::note("reload: models reloaded");
        let rehollowed = !hollows.is_empty();
        self.rehollow.borrow_mut().extend(hollows);
        self.hollow_waiting();
        self.say(if rehollowed {
            ctr!("Reload", "Reloaded {} from disk; hollowing it again.", name)
        } else {
            ctr!("Reload", "Reloaded {} from disk.", name)
        });
    }

    /// Hollows again the models waiting for it, one at a time.
    fn hollow_waiting(&self) {
        let next = self.rehollow.borrow().first().cloned();
        let Some((leaf, mesh, record)) = next else {
            return;
        };
        let still = self
            .document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .leaf_mesh_arc(leaf)
                    .is_some_and(|m| Arc::ptr_eq(m, &mesh))
            })
            .unwrap_or(false);
        if !still {
            self.rehollow.borrow_mut().remove(0);
            return;
        }
        if self.hollow.hollow_again(leaf, mesh, &record) {
            self.rehollow.borrow_mut().remove(0);
        }
    }

    /// The models' files, as they change.
    fn watch(&self) {
        if !self.bus.snapshot().reload_on_change {
            return;
        }
        if self.ctrl.is_mid_gesture() || self.ui.upgrade().is_none_or(|ui| ui.get_loading()) {
            return;
        }
        for path in self.document.model_file_paths() {
            let Some(now) = stamp_of(&path) else {
                continue;
            };
            let seen = self.stamps.borrow().get(&path).copied();
            match seen {
                None => {
                    self.stamps.borrow_mut().insert(path, now);
                }
                Some(seen) if seen == now => {
                    self.changed.borrow_mut().remove(&path);
                }
                Some(_) => {
                    // Written again: once it holds still a moment, so a
                    // program still writing it is not read half way.
                    let held = self.changed.borrow().get(&path) == Some(&now);
                    if held {
                        self.changed.borrow_mut().remove(&path);
                        self.stamps.borrow_mut().insert(path.clone(), now);
                        self.start(path, false);
                    } else {
                        self.changed.borrow_mut().insert(path, now);
                    }
                }
            }
        }
    }

    fn say(&self, text: slint::SharedString) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_status_notice(text.clone());
        let weak = self.ui.clone();
        self.notice
            .start(slint::TimerMode::SingleShot, NOTICE_FOR, move || {
                if let Some(ui) = weak.upgrade()
                    && ui.get_status_notice() == text
                {
                    ui.set_status_notice(Default::default());
                }
            });
    }

    /// Lands what is in flight now, for tests.
    #[cfg(test)]
    pub(crate) fn land_now(&self) {
        let waited = std::time::Instant::now();
        while !self.jobs.borrow().is_empty() {
            assert!(
                waited.elapsed() < Duration::from_secs(30),
                "the file is read"
            );
            std::thread::sleep(Duration::from_millis(10));
            self.land_jobs();
        }
    }

    /// One look at the files, for tests.
    #[cfg(test)]
    pub(crate) fn watch_now(&self) {
        self.watch();
    }
}

#[cfg(test)]
#[path = "tests_reload_from_disk.rs"]
mod tests_reload_from_disk;
