/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! How long several models take from the pick to all on screen (#391), and
//! the longest the window's thread is held meanwhile: the controller as the
//! window drives it, with a real renderer, frame after frame.
//!
//! Ignored: it reads the files named in `BREADSLICER_IMPORT_TIMING`
//! (separated by `;`). Run it with
//! `cargo test -p sc-slicer-gui --profile quick-release tests_import_timing -- --ignored --nocapture`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::mesh_render::{DeviceLoss, DeviceToken, Renderer};
use crate::test_support::gpu_devices::shared_test_device;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::tests_model_import::{finish_loads, write_box};
use crate::viewport::ViewportController;

fn files() -> Option<Vec<PathBuf>> {
    let list = std::env::var("BREADSLICER_IMPORT_TIMING").ok()?;
    Some(list.split(';').map(PathBuf::from).collect())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Opens `paths` in `ctrl` and runs frames until every model is drawn;
/// prints when each count of models first showed and the longest call.
fn measure(ctrl: &ViewportController, paths: Vec<PathBuf>, expected: usize, label: &str) {
    let document = ctrl.document();
    let before = document
        .with_scene(|s| s.graph().reachable_leaf_count())
        .unwrap_or(0);
    let started = Instant::now();
    ctrl.open_models(paths);
    let mut longest = started.elapsed();
    let mut shown = Vec::new();
    let mut last = before;
    let deadline = started + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "the models show within 120 s");
        std::thread::sleep(Duration::from_millis(2));
        let call = Instant::now();
        ctrl.on_scene_load_finished();
        ctrl.handle_before_rendering();
        longest = longest.max(call.elapsed());
        if ctrl.upload_owed_for_test() {
            continue;
        }
        let now = document
            .with_scene(|s| s.graph().reachable_leaf_count())
            .unwrap_or(0);
        if now != last {
            shown.push((started.elapsed(), now - before));
            last = now;
        }
        if !ctrl.loader().is_busy() && now - before >= expected {
            break;
        }
    }
    let steps = shown
        .iter()
        .map(|(at, n)| format!("{n} at {:.0} ms", ms(*at)))
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!(
        "\n== {label}: {steps}; longest UI call {:.0} ms",
        ms(longest)
    );
}

fn controller_with_renderer() -> Option<Rc<ViewportController>> {
    let (device, queue) = shared_test_device()?;
    let ctrl = Rc::new(ViewportController::new_for_test());
    ctrl.register_document_listener();
    let token = DeviceToken::next();
    ctrl.loader().set_gpu(device.clone(), token, queue.clone());
    let renderer = Renderer::new(device, token, queue, DeviceLoss::default()).expect("a renderer");
    ctrl.set_renderer_for_test(Box::new(renderer));
    Some(ctrl)
}

#[test]
#[ignore = "reads the models named in BREADSLICER_IMPORT_TIMING"]
fn several_models_from_the_pick_to_all_on_screen() {
    let Some(paths) = files() else {
        eprintln!("BREADSLICER_IMPORT_TIMING names no files: skipped");
        return;
    };
    on_fresh_slint_thread(move || {
        let Some(ctrl) = controller_with_renderer() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let count = paths.len();
        measure(&ctrl, paths.clone(), count, "empty scene");

        let Some(ctrl) = controller_with_renderer() else {
            return;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let base = write_box(dir.path(), "base.stl", [20.0, 20.0, 10.0]);
        ctrl.open_models(vec![base]);
        finish_loads(&ctrl);
        ctrl.handle_before_rendering();
        measure(&ctrl, paths, count, "scene already open");
    });
}
