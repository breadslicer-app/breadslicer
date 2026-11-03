/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! Where the files a launch names open (#418): the launch's own windows, or
//! the running app's, when a later launch hands them over
//! ([`crate::single_instance`]).
//!
//! Handed-over models join the scene of the window focused last, as Import
//! model adds them: one job, one undo step, with the import's own progress,
//! cancel and autosave. A project opens in that window only while it is
//! empty, and in a new one otherwise, so a handover never asks to replace a
//! scene. A window still loading or importing is waited for: the files wait
//! their turn, and nothing already running is touched.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

use super::{AppState, DEVICE_LOST_TARGET};
use crate::launch_files;
use crate::mesh_loader::{LoadDispatch, decide_load_dispatch, is_project};

/// How often files handed over to a busy window check whether it is free.
const BUSY_RETRY: Duration = Duration::from_millis(200);

/// Takes files a later launch handed over, on the window's thread. `false`
/// when this thread has no app to take them.
pub(crate) fn receive(paths: Vec<PathBuf>) -> bool {
    let Some(app) = DEVICE_LOST_TARGET.with(|target| target.borrow().upgrade()) else {
        return false;
    };
    app.receive_handoff(paths);
    true
}

impl AppState {
    /// Opens the launch's files, `paths` (see [`launch_files`]): every model
    /// in one window, each project in a window of its own. With no file, one
    /// empty window, whose id comes back.
    pub(crate) fn open_launch_files(
        self: &Rc<Self>,
        paths: Vec<PathBuf>,
    ) -> Result<Option<u64>, Box<dyn std::error::Error>> {
        let groups = launch_files::groups(paths);
        if groups.is_empty() {
            return Ok(Some(self.spawn_main_window()?));
        }
        crate::breadcrumbs::note(&format!("launch: {} group(s) of files", groups.len()));
        for group in groups {
            // The paths wait on the window's `PendingLoad` slot until
            // `RenderingSetup` publishes its GPU device.
            let window_id = self.spawn_main_window()?;
            self.open_paths_in_window(window_id, group);
        }
        Ok(None)
    }

    /// Records that window `id` took the focus: handed-over files go there.
    pub(super) fn note_focus(&self, id: u64) {
        self.focused_window.set(Some(id));
    }

    /// Queues the files a later launch handed over, brings the window they
    /// go to to the front and opens what it can take now.
    pub(crate) fn receive_handoff(self: &Rc<Self>, paths: Vec<PathBuf>) {
        crate::breadcrumbs::note(&format!("launch: {} file(s) handed over", paths.len()));
        self.handoffs
            .borrow_mut()
            .extend(launch_files::groups(paths));
        if let Some(id) = self.handoff_window() {
            self.raise_window(id);
        }
        self.open_handoffs();
    }

    /// The window handed-over files go to: the one focused last, or else the
    /// newest still open.
    fn handoff_window(&self) -> Option<u64> {
        let windows = self.main_windows.borrow();
        self.focused_window
            .get()
            .filter(|id| windows.contains_key(id))
            .or_else(|| windows.keys().max().copied())
    }

    /// Whether window `id` can start opening files now: it loads or imports
    /// nothing, and, without a scene, has the graphics device to open one
    /// with (until then, its queue holds the files of one request only).
    fn takes_files_now(&self, id: u64) -> bool {
        self.main_windows.borrow().get(&id).is_some_and(|wired| {
            let loader = wired.controller.loader();
            !loader.is_busy()
                && (wired.controller.document().is_loaded()
                    || decide_load_dispatch(loader.graphics_unavailable()) != LoadDispatch::Queue)
        })
    }

    /// Opens the queued handed-over files, in order, as far as their windows
    /// take them; the rest wait for a busy window to be free.
    pub(super) fn open_handoffs(self: &Rc<Self>) {
        loop {
            let Some(group) = self.handoffs.borrow().front().cloned() else {
                return;
            };
            let project = matches!(group.as_slice(), [path] if is_project(path));
            let target = self
                .handoff_window()
                .filter(|&id| !project || !self.shows_any(id));
            let window_id = match target {
                Some(id) if self.takes_files_now(id) => id,
                Some(_) => {
                    self.retry_handoffs();
                    return;
                }
                None => match self.spawn_main_window() {
                    Ok(id) => id,
                    Err(err) => {
                        eprintln!("Could not open a window for files handed over: {err}");
                        crate::breadcrumbs::note("launch: no window for files handed over");
                        self.handoffs.borrow_mut().pop_front();
                        continue;
                    }
                },
            };
            self.handoffs.borrow_mut().pop_front();
            self.open_paths_in_window(window_id, group);
            self.raise_window(window_id);
        }
    }

    /// Whether window `id` has a scene open (a project replaces it).
    fn shows_any(&self, id: u64) -> bool {
        self.main_windows
            .borrow()
            .get(&id)
            .is_some_and(|wired| wired.controller.document().is_loaded())
    }

    /// Tries the queued files again shortly; one timer at a time.
    fn retry_handoffs(self: &Rc<Self>) {
        if self.handoff_retry.replace(true) {
            return;
        }
        let app = Rc::downgrade(self);
        slint::Timer::single_shot(BUSY_RETRY, move || {
            if let Some(app) = app.upgrade() {
                app.handoff_retry.set(false);
                app.open_handoffs();
            }
        });
    }

    /// Brings window `id` to the front, restored if it was minimized.
    fn raise_window(&self, id: u64) {
        let Some(window) = self
            .main_windows
            .borrow()
            .get(&id)
            .map(|wired| wired.window.clone_strong())
        else {
            return;
        };
        window.window().with_winit_window(|winit_window| {
            winit_window.set_minimized(false);
            winit_window.focus_window();
        });
    }
}
